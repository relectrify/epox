use crate::{
    queue::{Queue, QueueEntry, Queueable},
    task,
    task::Task,
};
use nix::sys::epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags, EpollTimeout};
use pin_project::pin_project;
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
    fn queue_entry(self: Pin<&mut Self>) -> Pin<&mut QueueEntry>;
    fn as_any(&self) -> &dyn Any;
}

thread_local! {
    /**
     * Each thread gets a thread local executor which owns an epoll instance.
     */
    pub(crate) static EXECUTOR: Pin<Box<Executor>> = Executor::new().unwrap();
}

/*
 * The thing we use to hold on to a task.
 *
 * TODO: should be Pin<ThinRc<dyn AnyTask>>, then remove this type.
 */
type TaskBox = Box<RefCell<dyn AnyTask>>;
pub(crate) type TaskRef = Pin<Rc<TaskBox>>;

impl Queueable for TaskBox {
    fn with_entry<F: FnMut(Pin<&mut QueueEntry>)>(self: Pin<&Self>, mut f: F) {
        let mut s = self.borrow_mut();
        let s = unsafe { Pin::new_unchecked(&mut *s) };
        f(s.queue_entry());
    }
}

#[derive(Clone, Copy)]
enum TaskControlFlow {
    Normal,
    Yield,
    Shutdown,
}

/**
 * Executor
 */
#[pin_project]
pub(crate) struct Executor {
    epoll: RefCell<Epoll>,
    /* All tasks in the runq have either been newly spawned or have have been
     * woken. Either way, they need to be polled. */
    #[pin]
    runq_high: RefCell<Queue<TaskBox>>,
    #[pin]
    runq_normal: RefCell<Queue<TaskBox>>,
    #[pin]
    runq_low: RefCell<Queue<TaskBox>>,
    #[pin]
    sleepq: RefCell<Queue<TaskBox>>,
    events: RefCell<Vec<EpollEvent>>,
    task_control_flow: Cell<TaskControlFlow>,
    _not_send_not_sync: PhantomData<*mut ()>,
}

fn create_epoll() -> Result<Epoll, Error> {
    Ok(Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC)?)
}

impl Executor {
    fn new() -> Result<Pin<Box<Self>>, Error> {
        let s = Box::pin(Self {
            epoll: RefCell::new(create_epoll()?),
            runq_high: RefCell::new(unsafe { Queue::new() }),
            runq_normal: RefCell::new(unsafe { Queue::new() }),
            runq_low: RefCell::new(unsafe { Queue::new() }),
            sleepq: RefCell::new(unsafe { Queue::new() }),
            events: RefCell::new(Vec::new()),
            task_control_flow: Cell::new(TaskControlFlow::Normal),
            _not_send_not_sync: PhantomData,
        });
        unsafe {
            let mut runq = s.runq_high.borrow_mut();
            let runq = Pin::new(&mut *runq);
            runq.init();

            let mut runq = s.runq_normal.borrow_mut();
            let runq = Pin::new(&mut *runq);
            runq.init();

            let mut runq = s.runq_low.borrow_mut();
            let runq = Pin::new(&mut *runq);
            runq.init();

            let mut sleepq = s.sleepq.borrow_mut();
            let sleepq = Pin::new(&mut *sleepq);
            sleepq.init();
        }
        Ok(s)
    }

    fn spawn<T: 'static, F: Future<Output = T> + 'static>(
        self: Pin<&Self>,
        future: F,
        priority: Priority,
    ) -> crate::task::Handle<T, F> {
        let task: TaskRef = Rc::pin(Box::new(RefCell::new(Task::new(future, priority))));
        self.enqueue(task.clone());
        crate::task::Handle::new(task)
    }

    fn run(self: Pin<&Self>) -> Result<(), Error> {
        let this = self.project_ref();
        loop {
            while let Some(t) = self.highest_priority_runnable_task() {
                self.task_control_flow.set(TaskControlFlow::Normal);
                let pending = {
                    /* poll the task */
                    let waker = build_task_waker(&t);
                    let mut t = t.borrow_mut();
                    /* safety: t is pinned therefore its deref is also pinned */
                    let t = unsafe { Pin::new_unchecked(&mut *t) };
                    let mut cx = Context::from_waker(&waker);
                    t.poll(&mut cx) == Poll::Pending
                };
                match self.task_control_flow.get() {
                    TaskControlFlow::Normal => {
                        if pending {
                            /* task pending: put it on the sleep queue */
                            let mut sleepq = this.sleepq.borrow_mut();
                            let sleepq = Pin::new(&mut *sleepq);
                            sleepq.push(t);
                        }
                    }
                    TaskControlFlow::Yield => {
                        /* check for events */
                        self.epoll_wait(EpollTimeout::ZERO)?;
                        /* task yielded: put it back on a run queue */
                        {
                            let runq = self.runq(t.borrow().priority());
                            let mut runq = runq.borrow_mut();
                            let runq = Pin::new(&mut *runq);
                            runq.push(t);
                        }
                    }
                    TaskControlFlow::Shutdown => { /* shutting down */ }
                }
            }
            if self.sleepq.borrow().is_empty() {
                /* all tasks have completed */
                break;
            }

            /* wait for events */
            self.epoll_wait(EpollTimeout::NONE)?;
        }
        Ok(())
    }

    pub(crate) fn yield_now(&self) {
        self.task_control_flow.set(TaskControlFlow::Yield);
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
            EpollEvent::new(events, std::ptr::from_ref(Pin::into_inner(ew)) as _),
        )?;
        Ok(())
    }

    pub(crate) fn epoll_del(&self, fd: impl AsFd) -> Result<(), Error> {
        self.events.borrow_mut().pop();
        self.epoll.borrow().delete(fd)?;
        Ok(())
    }

    fn runq(self: Pin<&Self>, priority: Priority) -> Pin<&RefCell<Queue<TaskBox>>> {
        let this = self.project_ref();
        match priority {
            Priority::High => this.runq_high,
            Priority::Normal => this.runq_normal,
            Priority::Low => this.runq_low,
        }
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
            /* safety: e.data() is the result of std::ptr::from_ref(Pin::into_inner) */
            let ew = Pin::new(unsafe { &*(e.data() as *mut RefCell<EpollWaker>) });
            let mut waker = ew.borrow_mut();
            waker.events |= e.events();
            waker.waker.wake_by_ref();
        }
        Ok(())
    }

    fn enqueue(self: Pin<&Self>, task: TaskRef) {
        let runq = self.runq(task.borrow().priority());
        let mut runq = runq.borrow_mut();
        let runq = Pin::new(&mut *runq);
        runq.push(task);
    }

    fn highest_priority_runnable_task(self: Pin<&Self>) -> Option<TaskRef> {
        self.runq_pop(Priority::High)
            .or_else(|| self.runq_pop(Priority::Normal))
            .or_else(|| self.runq_pop(Priority::Low))
    }

    fn runq_pop(self: Pin<&Self>, priority: Priority) -> Option<TaskRef> {
        let runq = self.runq(priority);
        let mut runq = runq.borrow_mut();
        let runq = Pin::new(&mut *runq);
        runq.pop()
    }

    fn shutdown(self: Pin<&Self>) {
        while let Some(_task) = self.highest_priority_runnable_task() { /* drop task */ }

        /* gross */
        let mut sleepq = self.sleepq.borrow_mut();
        let mut sleepq = Pin::new(&mut *sleepq);
        while let Some(_task) = sleepq.as_mut().pop() { /* drop task */ }

        self.events.borrow_mut().clear();
        // old epoll will close on drop
        self.epoll.replace_with(|_| create_epoll().unwrap());

        self.task_control_flow.set(TaskControlFlow::Shutdown);
    }
}

/*
 * Build a waker for a task.
 */
fn build_task_waker(task: &TaskRef) -> Waker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);

    const unsafe fn clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &VTABLE)
    }

    unsafe fn wake(p: *const ()) {
        let task = unsafe { &Pin::new_unchecked(Rc::from_raw(p.cast::<TaskBox>())) };
        EXECUTOR.with(|e| e.as_ref().enqueue(task.clone()));
    }

    unsafe fn wake_by_ref(p: *const ()) {
        unsafe {
            wake(p);
        }
    }

    const unsafe fn drop(_p: *const ()) {}

    unsafe {
        Waker::from_raw(RawWaker::new(
            std::ptr::from_ref(task.as_ref().get_ref()).cast(),
            &VTABLE,
        ))
    }
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
 * Spawn a task on the thread local executor with normal priority that will
 * stop the executor if it returns [`Err`]
 *
 * See [spawn_checked_with_priority].
 */
pub fn spawn_checked<T: 'static, E: 'static, F: Future<Output = Result<T, E>> + 'static>(
    future: F,
) -> task::Handle<Result<T, E>, impl Future<Output = Result<T, E>>> {
    spawn_checked_with_priority(future, Priority::Normal)
}

/**
 * Spawn a task on the thread local executor with [`Priority`] priority that
 * will stop the executor if it returns [`Err`].
 *
 * This is a wrapper over [`spawn_with_priority`] that calls
 * [`shutdown_executor_unchecked()`] if the spawned future returns [`Err`].
 */
pub fn spawn_checked_with_priority<
    T: 'static,
    E: 'static,
    F: Future<Output = Result<T, E>> + 'static,
>(
    future: F,
    priority: Priority,
) -> task::Handle<Result<T, E>, impl Future<Output = Result<T, E>>> {
    spawn_with_priority(
        async move {
            future
                .await
                .inspect_err(|_| unsafe { shutdown_executor_unchecked() })
        },
        priority,
    )
}

/**
 * Spawn a task on the thread local executor with normal priority.
 */
pub fn spawn<T: 'static, F: Future<Output = T> + 'static>(future: F) -> task::Handle<T, F> {
    EXECUTOR.with(|e| e.as_ref().spawn(future, Priority::Normal))
}

/**
 * Spawn a task on the thread local executor with [`Priority`] priority.
 */
pub fn spawn_with_priority<T: 'static, F: Future<Output = T> + 'static>(
    future: F,
    priority: Priority,
) -> task::Handle<T, F> {
    EXECUTOR.with(|e| e.as_ref().spawn(future, priority))
}

/**
 * Run the thread local executor until no futures are pending.
 */
pub fn run() -> Result<(), std::io::Error> {
    EXECUTOR.with(|e| e.as_ref().run())
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
    EXECUTOR.with(|e| e.as_ref().shutdown());
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
