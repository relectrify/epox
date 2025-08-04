use std::{
    any::Any,
    cell::RefCell,
    io::Error,
    marker::PhantomData,
    ops::DerefMut,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

use crate::{Priority, Task, map_libc_error};

/**
 * A type erased task.
 */
pub trait AnyTask {
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()>;
    fn priority(&self) -> Priority;
    fn set_pending(&mut self);
    fn take_pending(&mut self) -> bool;
    fn as_any(&self) -> &dyn Any;
}

thread_local! {
    /**
     * Each thread gets a thread local executor which owns an epoll instance.
     */
    pub static EXECUTOR: Executor = Executor::new().unwrap();
}

/*
 * The thing we use to hold on to a task.
 *
 * TODO: should be Pin<ThinRc<dyn AnyTask>>, then remove this type.
 */
pub(crate) type TaskRef = Pin<Rc<Box<RefCell<dyn AnyTask>>>>;

impl std::ops::Index<Priority> for [RefCell<Vec<TaskRef>>; 3] {
    type Output = RefCell<Vec<TaskRef>>;

    fn index(&self, priority: Priority) -> &Self::Output {
        match priority {
            Priority::High => &self[0],
            Priority::Normal => &self[1],
            Priority::Low => &self[2],
        }
    }
}

impl std::ops::IndexMut<Priority> for [RefCell<Vec<TaskRef>>; 3] {
    fn index_mut(&mut self, priority: Priority) -> &mut Self::Output {
        match priority {
            Priority::High => &mut self[0],
            Priority::Normal => &mut self[1],
            Priority::Low => &mut self[2],
        }
    }
}

/**
 * Executor
 */
pub struct Executor {
    epoll_fd: libc::c_int,
    /* All tasks in the runq have either been newly spawned or have have been
     * woken. Either way, they need to be polled. */
    runq: [RefCell<Vec<TaskRef>>; 3],
    _not_send_not_sync: PhantomData<*mut ()>,
}

impl Executor {
    fn new() -> Result<Self, Error> {
        /* safety: we check the return value */
        let epoll_fd = map_libc_error(unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) })?;
        Ok(Self {
            epoll_fd,
            runq: std::array::from_fn(|_| RefCell::new(Vec::new())),
            _not_send_not_sync: PhantomData,
        })
    }

    pub fn spawn<T: 'static, F: Future<Output = T> + 'static>(
        &self,
        future: F,
        priority: Priority,
    ) -> crate::task::Handle<T, F> {
        let task: TaskRef = Rc::pin(Box::new(RefCell::new(Task::new(future, priority))));
        self.enqueue(task.clone());
        crate::task::Handle::new(task)
    }

    pub fn run(&self) -> Result<(), Error> {
        loop {
            let mut pending = false;
            while let Some(q) = self.highest_priority_non_empty_runq() {
                let t = q.borrow_mut().pop().unwrap();
                let waker = build_task_waker(t.clone());
                let mut cx = Context::from_waker(&waker);
                let mut t = t.borrow_mut();
                t.set_pending();
                /* safety: t is pinned therefore its deref is also pinned */
                let mut t = unsafe { Pin::new_unchecked(t.deref_mut()) };
                pending = t.as_mut().poll(&mut cx) == Poll::Pending || pending;
            }
            if !pending {
                /* all tasks have completed */
                break;
            }

            /* wait for events */
            /* TODO: Dynamically size. Needs to be big enough to receive all events at once. */
            let mut events = std::mem::MaybeUninit::<[libc::epoll_event; 64]>::uninit();
            let n_events = map_libc_error(unsafe {
                libc::epoll_wait(self.epoll_fd, events.assume_init_mut().as_mut_ptr(), 64, -1)
            })? as usize;
            let events = unsafe { &events.assume_init()[..n_events] };
            /* wakeup futures which have an event, adding them to runq */
            for e in events {
                let ew = unsafe { Rc::<RefCell<EpollWaker>>::from_raw(e.u64 as *const _) };
                let mut waker = ew.borrow_mut();
                waker.event = true;
                waker.waker.wake_by_ref();
            }
        }
        Ok(())
    }

    pub fn epoll_add(
        &self,
        fd: libc::c_int,
        events: i32,
        ew: Pin<Rc<RefCell<EpollWaker>>>,
    ) -> Result<(), Error> {
        let mut event = libc::epoll_event {
            events: events as u32,
            /* safety: the pinned object remains pinned, we don't move it */
            u64: Rc::<RefCell<EpollWaker>>::into_raw(unsafe { Pin::into_inner_unchecked(ew) }) as _,
        };
        /* safety: we check the return value */
        map_libc_error(unsafe {
            libc::epoll_ctl(self.epoll_fd, libc::EPOLL_CTL_ADD, fd, &mut event)
        })?;
        Ok(())
    }

    pub fn epoll_del(&self, fd: libc::c_int) -> Result<(), Error> {
        /* safety: we check the return value */
        map_libc_error(unsafe {
            libc::epoll_ctl(self.epoll_fd, libc::EPOLL_CTL_DEL, fd, std::ptr::null_mut())
        })?;
        Ok(())
    }

    fn enqueue(&self, task: TaskRef) {
        if !task.borrow_mut().take_pending() {
            return;
        }
        let priority = task.borrow().priority();
        self.runq[priority].borrow_mut().push(task);
    }

    fn highest_priority_non_empty_runq(&self) -> Option<&RefCell<Vec<TaskRef>>> {
        if !self.runq[Priority::High].borrow().is_empty() {
            return Some(&self.runq[Priority::High]);
        }
        if !self.runq[Priority::Normal].borrow().is_empty() {
            return Some(&self.runq[Priority::Normal]);
        }
        if !self.runq[Priority::Low].borrow().is_empty() {
            return Some(&self.runq[Priority::Low]);
        }
        None
    }
}

/*
 * Buile a waker for a task.
 */
fn build_task_waker(task: TaskRef) -> Waker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);

    unsafe fn clone(p: *const ()) -> RawWaker {
        /* safety: p is the result of Rc::<Box<RefCell<dyn AnyTask>>>::into_raw */
        unsafe { Rc::increment_strong_count(p as *const Box<RefCell<dyn AnyTask>>) };
        RawWaker::new(p, &VTABLE)
    }

    unsafe fn wake(p: *const ()) {
        /* safety: p is the result of Rc::<Box<RefCell<dyn AnyTask>>>::into_raw */
        let rc = unsafe { Rc::<Box<RefCell<dyn AnyTask>>>::from_raw(p as *const _) };
        /* safety: rc is the result of Pin::into_inner_unchecked */
        let task = unsafe { Pin::new_unchecked(rc) };
        EXECUTOR.with(|e| e.enqueue(task));
    }

    unsafe fn wake_by_ref(p: *const ()) {
        unsafe {
            /* safety: p is the result of Rc::<Box<RefCell<dyn AnyTask>>>::into_raw */
            Rc::increment_strong_count(p as *const Box<RefCell<dyn AnyTask>>);
            /* safety: wake consumes p, but we have incremented the strong
             * count so wake_by_ref semantics are met */
            wake(p);
        }
    }

    unsafe fn drop(p: *const ()) {
        unsafe { Rc::decrement_strong_count(p as *const Box<RefCell<dyn AnyTask>>) };
    }

    let raw_waker = RawWaker::new(
        /* safety: the pinned object remains pinned, we don't move it */
        Rc::<Box<RefCell<dyn AnyTask>>>::into_raw(unsafe { Pin::into_inner_unchecked(task) })
            as *const (),
        &VTABLE,
    );

    unsafe { Waker::from_raw(raw_waker) }
}

/**
 * All events registered with epoll hold a reference to an EpollWaker in their
 * associated data.
 */
pub struct EpollWaker {
    waker: Waker,
    event: bool,
}

impl EpollWaker {
    /**
     * Check if this EpollWaker instance has had an event since it was last
     * polled.
     */
    pub fn poll(&mut self, cx: &mut std::task::Context<'_>) -> bool {
        if !self.event {
            self.waker = cx.waker().clone();
            return false;
        }
        self.event = false;
        return true;
    }
}

impl Default for EpollWaker {
    fn default() -> Self {
        Self {
            waker: Waker::noop().clone(),
            event: false,
        }
    }
}
