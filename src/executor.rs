use crate::{task, task::Task};
use nix::sys::epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags, EpollTimeout};
use std::{
    any::Any,
    cell::{Cell, RefCell},
    io::Error,
    marker::PhantomData,
    os::fd::AsFd,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

/**
 * A type erased task.
 */
pub(crate) trait AnyTask {
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
    pub(crate) static EXECUTOR: Executor = Executor::new().unwrap();
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
pub(crate) struct Executor {
    epoll: RefCell<Epoll>,
    /* All tasks in the runq have either been newly spawned or have have been
     * woken. Either way, they need to be polled. */
    runq: [RefCell<Vec<TaskRef>>; 3],
    events: RefCell<Vec<EpollEvent>>,
    task_yielded: Cell<bool>,
    _not_send_not_sync: PhantomData<*mut ()>,
}

fn create_epoll() -> Result<Epoll, Error> {
    Ok(Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC)?)
}

impl Executor {
    fn new() -> Result<Self, Error> {
        Ok(Self {
            epoll: RefCell::new(create_epoll()?),
            runq: std::array::from_fn(|_| RefCell::new(Vec::new())),
            events: RefCell::new(Vec::new()),
            task_yielded: Cell::new(false),
            _not_send_not_sync: PhantomData,
        })
    }

    fn spawn<T: 'static, F: Future<Output = T> + 'static>(
        &self,
        future: F,
        priority: Priority,
    ) -> crate::task::Handle<T, F> {
        let task: TaskRef = Rc::pin(Box::new(RefCell::new(Task::new(future, priority))));
        self.enqueue(task.clone());
        crate::task::Handle::new(task)
    }

    fn run(&self) -> Result<(), Error> {
        loop {
            let mut pending = false;
            while let Some(q) = self.highest_priority_non_empty_runq() {
                let t = q.borrow_mut().pop().unwrap();
                self.task_yielded.set(false);
                {
                    /* poll the task */
                    let waker = build_task_waker(t.clone());
                    let mut cx = Context::from_waker(&waker);
                    let mut t = t.borrow_mut();
                    t.set_pending();
                    /* safety: t is pinned therefore its deref is also pinned */
                    let mut t = unsafe { Pin::new_unchecked(&mut *t) };
                    pending = t.as_mut().poll(&mut cx) == Poll::Pending || pending;
                }
                if self.task_yielded.get() {
                    /* task yielded: insert task at the front of the queue so
                     * that all other tasks run before it */
                    let priority = t.borrow().priority();
                    self.runq[priority].borrow_mut().insert(0, t);
                    /* check for events */
                    self.epoll_wait(EpollTimeout::ZERO)?;
                }
            }
            if !pending && self.events.borrow().is_empty() {
                /* all tasks have completed */
                break;
            }

            /* wait for events */
            self.epoll_wait(EpollTimeout::NONE)?;
        }
        Ok(())
    }

    pub(crate) fn yield_now(&self) {
        self.task_yielded.set(true);
    }

    pub(crate) fn epoll_add(
        &self,
        fd: impl AsFd,
        events: EpollFlags,
        ew: Pin<&RefCell<EpollWaker>>,
    ) -> Result<(), Error> {
        self.events.borrow_mut().push(EpollEvent::empty());
        self.epoll.borrow().add(
            fd,
            EpollEvent::new(
                events,
                /* safety: the pinned object remains pinned, we don't move it */
                std::ptr::from_ref(unsafe { Pin::into_inner_unchecked(ew) }) as _,
            ),
        )?;
        Ok(())
    }

    pub(crate) fn epoll_del(&self, fd: impl AsFd) -> Result<(), Error> {
        self.events.borrow_mut().pop();
        self.epoll.borrow().delete(fd)?;
        Ok(())
    }

    fn epoll_wait(&self, timeout: EpollTimeout) -> Result<(), Error> {
        if self.events.borrow().is_empty() {
            return Ok(());
        }
        let n_events = self
            .epoll
            .borrow()
            .wait(self.events.borrow_mut().as_mut_slice(), timeout)?;
        /* wakeup futures which have an event, adding them to runq */
        for e in &self.events.borrow()[0..n_events] {
            /* safety: e.data() is the result of Pin::into_inner_unchecked */
            let ew = unsafe { Pin::new_unchecked(&*(e.data() as *mut RefCell<EpollWaker>)) };
            let mut waker = ew.borrow_mut();
            waker.events |= e.events();
            waker.waker.wake_by_ref();
        }
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

    fn shutdown(&self) {
        while let Some(queue) = self.highest_priority_non_empty_runq() {
            queue.borrow_mut().clear();
        }
        self.events.borrow_mut().clear();
        // old epoll will close on drop
        self.epoll.replace_with(|_| create_epoll().unwrap());
    }
}

/*
 * Buile a waker for a task.
 */
fn build_task_waker(task: TaskRef) -> Waker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);

    unsafe fn clone(p: *const ()) -> RawWaker {
        /* safety: p is the result of Rc::<Box<RefCell<dyn AnyTask>>>::into_raw */
        unsafe { Rc::increment_strong_count(p.cast::<Box<RefCell<dyn AnyTask>>>()) };
        RawWaker::new(p, &VTABLE)
    }

    unsafe fn wake(p: *const ()) {
        /* safety: p is the result of Rc::<Box<RefCell<dyn AnyTask>>>::into_raw */
        let rc = unsafe { Rc::<Box<RefCell<dyn AnyTask>>>::from_raw(p.cast()) };
        /* safety: rc is the result of Pin::into_inner_unchecked */
        let task = unsafe { Pin::new_unchecked(rc) };
        EXECUTOR.with(|e| e.enqueue(task));
    }

    unsafe fn wake_by_ref(p: *const ()) {
        unsafe {
            /* safety: p is the result of Rc::<Box<RefCell<dyn AnyTask>>>::into_raw */
            Rc::increment_strong_count(p.cast::<Box<RefCell<dyn AnyTask>>>());
            /* safety: wake consumes p, but we have incremented the strong
             * count so wake_by_ref semantics are met */
            wake(p);
        }
    }

    unsafe fn drop(p: *const ()) {
        unsafe { Rc::decrement_strong_count(p.cast::<Box<RefCell<dyn AnyTask>>>()) };
    }

    let raw_waker = RawWaker::new(
        /* safety: the pinned object remains pinned, we don't move it */
        Rc::<Box<RefCell<dyn AnyTask>>>::into_raw(unsafe { Pin::into_inner_unchecked(task) })
            .cast::<()>(),
        &VTABLE,
    );

    unsafe { Waker::from_raw(raw_waker) }
}

/**
 * All events registered with epoll hold a reference to an EpollWaker in
 * their associated data.
 */
pub(crate) struct EpollWaker {
    waker: Waker,
    events: EpollFlags,
    _not_send_not_sync: core::marker::PhantomData<*mut ()>,
}

impl EpollWaker {
    /**
     * Check if this EpollWaker instance has had any events since it was
     * last polled.
     */
    pub(crate) fn poll(&mut self, cx: &std::task::Context<'_>) -> Poll<EpollFlags> {
        self.waker.clone_from(cx.waker());
        if self.events.is_empty() {
            Poll::Pending
        } else {
            Poll::Ready(std::mem::replace(&mut self.events, EpollFlags::empty()))
        }
    }
}

impl Default for EpollWaker {
    fn default() -> Self {
        Self {
            waker: Waker::noop().clone(),
            events: EpollFlags::empty(),
            _not_send_not_sync: core::marker::PhantomData,
        }
    }
}

/**
 * Task priority.
 *
 * Tasks are run from high to low priority.
 */
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Priority {
    High,
    Normal,
    Low,
}

/**
 * Spawn a task on the thread local executor with normal priority.
 */
pub fn spawn<T: 'static, F: Future<Output = T> + 'static>(future: F) -> task::Handle<T, F> {
    EXECUTOR.with(|e| e.spawn(future, Priority::Normal))
}

/**
 * Spawn a task on the thread local executor with [`Priority`] priority.
 */
pub fn spawn_with_priority<T: 'static, F: Future<Output = T> + 'static>(
    future: F,
    priority: Priority,
) -> task::Handle<T, F> {
    EXECUTOR.with(|e| e.spawn(future, priority))
}

/**
 * Run the thread local executor until no futures are pending.
 */
pub fn run() -> Result<(), std::io::Error> {
    EXECUTOR.with(|e| e.run())
}

/**
 * Removes all pending tasks from the executor and returns control to the
 * caller.
 *
 * This will cause [`epox::run()`] to return as soon as control is returned
 * to the executor.
 *
 * If you _don't_ need to return a value from the task that shuts down the
 * executor, you should probably use [`shutdown()`].
 *
 * # Safety
 *
 * Callers __should not__ call `.await` after calling this function. This
 * will return control to the executor which __will not__ wake previously
 * registered tasks.
 *
 * [`epox::run()`]: run
 */
pub unsafe fn shutdown_executor_unchecked() {
    EXECUTOR.with(|e| e.shutdown());
}

/**
 * Stop the executor, deregistering all tasks.
 *
 * All tasks, including the current task, will be stopped once this future
 * is polled.
 *
 * The [`task::Handle`] of the current task _will not_ contain a result.
 * Tasks that need to shut down the executor _and_ return a value should
 * call [`shutdown_executor_unchecked()`].
 */
pub const fn shutdown() -> Shutdown {
    Shutdown { _private: () }
}

#[must_use]
pub struct Shutdown {
    _private: (),
}

impl Future for Shutdown {
    #[cfg(feature = "nightly")]
    type Output = !;
    #[cfg(not(feature = "nightly"))]
    type Output = std::convert::Infallible;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe { shutdown_executor_unchecked() };
        Poll::Pending
    }
}
