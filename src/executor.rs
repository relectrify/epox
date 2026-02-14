use crate::{
    queue::{Queue, QueueEntry, Queueable},
    task,
    task::Task,
};
use nix::{
    fcntl::OFlag,
    sys::epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags, EpollTimeout},
    unistd::{pipe2, read, write},
};
use pin_project_lite::pin_project;
use std::{
    cell::{Cell, RefCell, UnsafeCell},
    io::Error,
    marker::PhantomData,
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd},
    pin::Pin,
    sync::{Arc, Weak},
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

thread_local! {
    /**
     * Each thread gets a thread local executor which owns an epoll instance.
     */
    pub(crate) static EXECUTOR: RefCell<Pin<Box<Executor>>> = RefCell::new(Executor::new().unwrap());
}

/*
 * Do something with the thread local executor.
 */
pub(crate) fn exec<R, F: FnOnce(Pin<&mut Executor>) -> R>(f: F) -> R {
    EXECUTOR.with_borrow_mut(|e| f(e.as_mut()))
}

/*
 * The thing the executor uses to hold on to a task.
 *
 * Unfortunately we currently need two heap allocations to store a task as
 * the waker can only store a thin pointer. One of these features needs to
 * stabilise for us to do any better here:
 *  https://doc.rust-lang.org/nightly/unstable-book/library-features/context-ext.html
 *  https://doc.rust-lang.org/beta/unstable-book/library-features/thin-box.html
 *  https://doc.rust-lang.org/beta/unstable-book/library-features/ptr-metadata.html
 * TODO: use one allocation for tasks
 */
pub(crate) struct ExecutorTask {
    queue_entry: UnsafeCell<QueueEntry>,
    /* write the task's address to this pipe to wake the task */
    pipe_wr: RawFd,
    /* use this waker to wake the task */
    waker: Waker,
    /* use this waker to wake a Handle which may be waiting on this task */
    pub(crate) handle_waker: Cell<Waker>,
    priority: Priority,
    pub(crate) task: Pin<Box<RefCell<Task>>>,
    /* keep track of whether this task has returned Poll::Ready, so we don't enqueue a task that
     * has completed */
    has_completed: Cell<bool>,
    /* this struct must be pinned as queue requires pinned entries */
    _pinned: std::marker::PhantomPinned,
}
pub(crate) type TaskRef = Pin<Arc<ExecutorTask>>;

impl Queueable for ExecutorTask {
    fn with_entry<R, F: FnMut(Pin<&mut QueueEntry>) -> R>(self: &Pin<Arc<Self>>, mut f: F) -> R {
        /* safety: self is pinned therefore queue_entry is pinned */
        f(unsafe { Pin::new_unchecked(&mut *self.queue_entry.get()) })
    }

    fn from_entry(entry: Pin<&QueueEntry>) -> Pin<Arc<Self>> {
        /* this optimises into a constant offset */
        let cell_inner_offset = {
            let cell = UnsafeCell::new(QueueEntry::new());
            let inner_p = cell.get().cast::<u8>();
            let cell_p = std::ptr::from_ref(&cell).cast::<u8>();
            /* safety: inner_p must be >= cell_p */
            unsafe { inner_p.offset_from_unsigned(cell_p) }
        };
        let queue_entry_offset = std::mem::offset_of!(Self, queue_entry) + cell_inner_offset;
        /* safety: pointer arithmetic :) */
        let task = unsafe {
            std::ptr::from_ref(&*entry.as_ref())
                .cast::<u8>()
                .sub(queue_entry_offset)
                .cast()
        };
        /* safety: task is an Arc whose contents do not move */
        unsafe { Pin::new_unchecked(Arc::from_raw(task)) }
    }
}

impl ExecutorTask {
    fn is_queued(self: &Pin<Arc<Self>>) -> bool {
        self.with_entry(|e| e.is_queued())
    }
}

fn create_epoll() -> Result<Epoll, Error> {
    Ok(Epoll::new(EpollCreateFlags::EPOLL_CLOEXEC)?)
}

pin_project! {
    /**
     * Executor
     */
    pub(crate) struct Executor {
        epoll: Epoll,
        /* All tasks in the run queues have either been newly spawned or have have
        * been woken. Either way, they need to be polled. */
        #[pin]
        runq_high: Queue<ExecutorTask>,
        #[pin]
        runq_normal: Queue<ExecutorTask>,
        #[pin]
        runq_low: Queue<ExecutorTask>,
        /* Tasks in the sleep queue are pending. */
        #[pin]
        sleepq: Queue<ExecutorTask>,
        /* If a task needs to be woken from another thread it's address is
        * written to this task to be handled by the local executor. */
        pipe_rd: OwnedFd,
        pipe_wr: OwnedFd,
        events: Vec<EpollEvent>,
        shutdown: bool,
        _not_send_not_sync: PhantomData<*mut ()>,
    }
}

impl Executor {
    fn new() -> Result<Pin<Box<Self>>, Error> {
        let (pipe_rd, pipe_wr) = pipe2(OFlag::O_CLOEXEC | OFlag::O_NONBLOCK)?;
        let mut s = Box::pin(Self {
            epoll: create_epoll()?,
            runq_high: unsafe { Queue::new() },
            runq_normal: unsafe { Queue::new() },
            runq_low: unsafe { Queue::new() },
            sleepq: unsafe { Queue::new() },
            pipe_rd,
            pipe_wr,
            events: Vec::new(),
            shutdown: false,
            _not_send_not_sync: PhantomData,
        });
        let this = s.as_mut().project();
        unsafe {
            this.runq_high.init();
            this.runq_normal.init();
            this.runq_low.init();
            this.sleepq.init();
        }
        /* task wakeup pipe */
        this.events.push(EpollEvent::empty());
        this.epoll
            .add(this.pipe_rd, EpollEvent::new(EpollFlags::EPOLLIN, 0))
            .unwrap();
        Ok(s)
    }

    fn spawn<F: Future + 'static>(
        self: Pin<&mut Self>,
        future: F,
        priority: Priority,
    ) -> crate::task::Handle<F::Output> {
        let task = Arc::new_cyclic(|me| ExecutorTask {
            queue_entry: UnsafeCell::new(QueueEntry::new()),
            pipe_wr: self.pipe_wr.as_raw_fd(),
            waker: task_waker::build(me),
            handle_waker: Cell::new(Waker::noop().clone()),
            priority,
            task: Task::new_pinned_boxed_refcell(future),
            has_completed: Cell::new(false),
            _pinned: std::marker::PhantomPinned,
        });
        /* safety: Arc is missing a way to build a pinned cyclic */
        let task = unsafe { Pin::new_unchecked(task) };
        self.enqueue(task.clone());
        crate::task::Handle::new(task)
    }

    pub(crate) fn abort(self: Pin<&mut Self>, task: TaskRef) {
        let this = self.project();
        this.sleepq.release(task);
    }

    pub(crate) fn epoll_add(
        self: Pin<&mut Self>,
        fd: impl AsFd,
        events: EpollFlags,
        ew: Pin<&RefCell<EpollWaker>>,
    ) -> Result<(), Error> {
        let this = self.project();
        this.events.push(EpollEvent::empty());
        this.epoll.add(
            fd,
            EpollEvent::new(events, std::ptr::from_ref(Pin::into_inner(ew)) as _),
        )?;
        Ok(())
    }

    pub(crate) fn epoll_del(self: Pin<&mut Self>, fd: impl AsFd) -> Result<(), Error> {
        let this = self.project();
        this.events.pop();
        this.epoll.delete(fd)?;
        Ok(())
    }

    fn runq(self: Pin<&mut Self>, priority: Priority) -> Pin<&mut Queue<ExecutorTask>> {
        let this = self.project();
        match priority {
            Priority::High => this.runq_high,
            Priority::Normal => this.runq_normal,
            Priority::Low => this.runq_low,
        }
    }

    fn epoll_wait(mut self: Pin<&mut Self>, timeout: EpollTimeout) -> Result<(), Error> {
        if self.events.is_empty() {
            return Ok(());
        }
        let n_events = {
            let this = self.as_mut().project();
            this.epoll
                .wait(this.events.as_mut_slice(), timeout)
                .or_else(|e| {
                    if e == nix::Error::EINTR {
                        /* epoll_wait returns EINTR even if the interrupting
                         * signal handler is set to SA_RESTART, ignore this */
                        Ok(0)
                    } else {
                        Err(e)
                    }
                })
        }?;
        /* wakeup futures which have an event, adding them to runq */
        for ei in 0..n_events {
            let e = self.events[ei];
            if e.data() == 0 {
                self.as_mut().wake_from_pipe()?;
                continue;
            }
            /* safety: e.data() is the result of std::ptr::from_ref(Pin::into_inner) */
            let ew = Pin::new(unsafe { &*(e.data() as *mut RefCell<EpollWaker>) });
            let mut waker = ew.borrow_mut();
            if waker.events.contains(e.events()) {
                // epoll might give us an edge even though the fd hasn't yet returned
                // EWOULDBLOCK/EAGAIN
                continue;
            }
            waker.events |= e.events();
            /* if the waker is a task waker, we know that data is a task */
            if *waker.waker.vtable() == task_waker::VTABLE {
                /* don't call waker.waker.wake_by_ref() as the executor is already borrowed! */
                let weak = weak_from_raw(waker.waker.data());
                if let Some(task) = weak.upgrade() {
                    /* safety: task is an Arc whose contents do not move */
                    let task = unsafe { Pin::new_unchecked(task) };
                    self.as_mut().enqueue(task);
                }
                let _ = Weak::into_raw(weak);
            }
        }
        Ok(())
    }

    fn enqueue(self: Pin<&mut Self>, task: TaskRef) {
        if !task.has_completed.get() {
            self.runq(task.priority).push(task);
        }
    }

    fn sleep(self: Pin<&mut Self>, task: TaskRef) {
        let this = self.project();
        this.sleepq.push(task);
    }

    fn highest_priority_runnable_task(self: Pin<&mut Self>) -> Option<TaskRef> {
        let this = self.project();
        this.runq_high
            .pop()
            .or_else(|| this.runq_normal.pop())
            .or_else(|| this.runq_low.pop())
    }

    fn shutdown(self: Pin<&mut Self>) {
        *self.project().shutdown = true;
    }

    fn wake_from_pipe(mut self: Pin<&mut Self>) -> Result<(), Error> {
        let mut buf = std::mem::MaybeUninit::<*const ()>::uninit();
        /* safety: read does not require an initialised buffer */
        read(&self.pipe_rd, unsafe {
            std::slice::from_raw_parts_mut(
                buf.as_mut_ptr().cast::<u8>(),
                std::mem::size_of_val(&buf),
            )
        })?;
        /* safety: if read() does not return an error, buf is initialised */
        let weak = weak_from_raw(unsafe { buf.assume_init() });
        /* wake up task */
        if let Some(task) = weak.upgrade() {
            /* safety: task is an Arc whose contents do not move */
            let task = unsafe { Pin::new_unchecked(task) };
            self.as_mut().enqueue(task);
        }
        Ok(())
    }
}

fn weak_from_raw(p: *const ()) -> Weak<ExecutorTask> {
    /* safety: p is the result of Weak::into_raw */
    unsafe { Weak::from_raw(p.cast::<ExecutorTask>()) }
}

/*
 * Build a waker for a task.
 */
mod task_waker {
    use super::{ExecutorTask, Weak, exec, weak_from_raw};
    use nix::sys::epoll::EpollTimeout;
    use nix::unistd::write;
    use std::{
        os::fd::{AsRawFd, BorrowedFd},
        pin::Pin,
        sync::Arc,
        task::{RawWaker, RawWakerVTable, Waker},
    };

    pub(super) fn build(task: &Weak<ExecutorTask>) -> Waker {
        unsafe { Waker::from_raw(RawWaker::new(Weak::into_raw(task.clone()).cast(), &VTABLE)) }
    }

    pub(super) const VTABLE: RawWakerVTable =
        RawWakerVTable::new(clone, wake_by_val, wake_by_ref, drop);

    fn wake(weak: &Weak<ExecutorTask>) {
        if let Some(task) = weak.upgrade() {
            exec(|mut e| {
                // Waker implements Send + Sync, so this needs to be able to
                // wake the task from any thread. Each executor listens for
                // tasks on a pipe, which the task stores in pipe_wr. We send
                // the task pointer into this pipe where the corresponding
                // thread will see it and enqueue the task.

                // If we're already on the same thread we don't need to go
                // through the kernel - we can directly enqueue the task.

                // A task is allowed to wake itself, see:
                // See https://doc.rust-lang.org/std/task/struct.Waker.html#method.wake
                if task.pipe_wr == e.pipe_wr.as_raw_fd() {
                    if task.task.try_borrow().is_err() {
                        /* task woke itself -- check for events now to make sure
                         * the task runs again after any new events */
                        let _ = e.as_mut().epoll_wait(EpollTimeout::ZERO);
                    }
                    /* safety: task is an Arc whose contents do not move */
                    let task = unsafe { Pin::new_unchecked(task) };
                    e.enqueue(task);
                } else {
                    let p = Weak::into_raw(weak.clone());
                    let bytes = (p as usize).to_ne_bytes();
                    /* safety: fd stays valid for duration of call */
                    match write(unsafe { BorrowedFd::borrow_raw(task.pipe_wr) }, &bytes) {
                        /* EBADF indicates the pipe has been closed - so the executor associated
                         * with this waker has stopped */
                        Ok(_) | Err(nix::errno::Errno::EBADF) => {}
                        Err(e) => panic!("error writing to task wake pipe: {e:?}"),
                    }
                }
            });
        }
    }

    unsafe fn clone(p: *const ()) -> RawWaker {
        let weak = weak_from_raw(p);
        let clone = if let Some(task) = weak.upgrade() {
            /* task still running */
            RawWaker::new(Weak::into_raw(Arc::downgrade(&task)).cast(), &VTABLE)
        } else {
            /* task has completed */
            RawWaker::new(Waker::noop().data(), Waker::noop().vtable())
        };
        let _ = Weak::into_raw(weak);
        clone
    }

    unsafe fn wake_by_val(p: *const ()) {
        wake(&weak_from_raw(p));
    }

    unsafe fn wake_by_ref(p: *const ()) {
        let weak = weak_from_raw(p);
        wake(&weak);
        let _ = Weak::into_raw(weak);
    }

    unsafe fn drop(p: *const ()) {
        weak_from_raw(p);
    }
}

/**
 * All events registered with epoll hold a reference to an EpollWaker in
 * their associated data.
 */
pub(crate) struct EpollWaker {
    waker: Waker,
    events: EpollFlags,
    /// Number of times [`poll_with`] has returned `Ready` since it last
    /// returned `Pending`
    ///
    /// [`poll_with`]: Self::poll_with
    burst: usize,
    _not_send_not_sync: core::marker::PhantomData<*mut ()>,
}

#[must_use]
pub(crate) enum ControlFlow<T> {
    Normal(Poll<T>),
    Yield,
}

impl EpollWaker {
    pub(crate) fn new(events: EpollFlags) -> Self {
        Self {
            waker: Waker::noop().clone(),
            events,
            burst: 0,
            _not_send_not_sync: PhantomData,
        }
    }
    /**
     * Handle events on this waker. Caller must return the result of the io
     * (usually read() or write()) operation which was run on the associated
     * file descriptor.
     */
    pub(crate) fn poll_with<T, F: FnMut(EpollFlags) -> Result<T, std::io::Error>>(
        &mut self,
        cx: &std::task::Context<'_>,
        mut func: F,
    ) -> ControlFlow<Result<T, std::io::Error>> {
        if self.events.is_empty() {
            self.burst = 0;
            self.waker.clone_from(cx.waker());
            return ControlFlow::Normal(Poll::Pending);
        }

        // TODO: make this configurable
        const BURST_LIMIT: usize = 20;

        // if we've returned Poll::Ready a few times in a row, manually yield to the
        // executor to give other tasks a turn. without this we could end up never
        // yielding if the fd is always ready
        if self.burst >= BURST_LIMIT {
            self.burst = 0;

            return ControlFlow::Yield;
        }
        loop {
            match func(self.events) {
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => { /* EINTR: try again */ }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    /* EAGAIN, EWOULDBLOCK: wait for event */
                    self.burst = 0;
                    self.waker.clone_from(cx.waker());
                    self.events = EpollFlags::empty();
                    break ControlFlow::Normal(Poll::Pending);
                }
                res => {
                    self.burst += 1;
                    break ControlFlow::Normal(Poll::Ready(res));
                }
            }
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
) -> task::Handle<Result<T, E>> {
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
) -> task::Handle<Result<T, E>> {
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
pub fn spawn<F: Future + 'static>(future: F) -> task::Handle<F::Output> {
    exec(|e| e.spawn(future, Priority::Normal))
}

/**
 * Spawn a task on the thread local executor with [`Priority`] priority.
 */
pub fn spawn_with_priority<F: Future + 'static>(
    future: F,
    priority: Priority,
) -> task::Handle<F::Output> {
    exec(|e| e.spawn(future, priority))
}

fn run_task(t: TaskRef) {
    let pending = {
        /* poll the task */
        let mut cx = Context::from_waker(&t.waker);
        let mut ti = t.task.borrow_mut();
        /* safety: t is pinned therefore its deref is also pinned */
        let ti = unsafe { Pin::new_unchecked(&mut *ti) };
        match ti.poll(&mut cx) {
            Poll::Pending => true,
            Poll::Ready(()) => {
                /* wakeup handle potentially waiting on this task */
                t.handle_waker.replace(Waker::noop().clone()).wake_by_ref();
                t.has_completed.replace(true);
                false
            }
        }
    };

    /* if task is still queued it yielded and is in a run queue */
    if pending && !t.is_queued() {
        /* task pending and not runnable: put it on the sleep queue */
        exec(|e| e.sleep(t));
    }
}

/**
 * Run the thread local executor until no futures are pending.
 */
pub fn run() -> Result<(), std::io::Error> {
    'outer: loop {
        while let Some(runnable_task) = exec(|e| e.highest_priority_runnable_task()) {
            run_task(runnable_task);

            if exec(|e| e.shutdown) {
                break 'outer;
            }

            /* check for events which happened while task was running */
            exec(|e| e.epoll_wait(EpollTimeout::ZERO))?;
        }

        if exec(|e| e.sleepq.is_empty()) {
            /* all tasks have completed */
            break;
        }
        /* wait for events */
        exec(|e| e.epoll_wait(EpollTimeout::NONE))?;
    }

    // drop the executor, releasing all resources it has used
    // replaces it with a new one so you can call run() again
    let _ = EXECUTOR.replace(Executor::new()?);

    Ok(())
}

/**
 * Runs a future to completion while allowing other tasks to run.
 *
 * This functions as a "blocking `.await`" - other tasks will be able to run
 * until this future returns [`Poll::Ready`].
 *
 * May be useful when implementing traits (an async [`Drop`] implementation)
 * or to run a single future in a blocking context.
 *
 * ```
 * fn blocking_function() {
 *     # use std::time::Duration;
 *     let result = epox::executor::block_on(
 *         Box::pin(async { epox::time::sleep(Duration::from_millis(100)).await }).as_mut(),
 *     );
 *     result.unwrap();
 * }
 * ```
 */
#[allow(
    clippy::must_use_candidate,
    reason = "we want to inherit the must_use of F::Output"
)]
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
            return v;
        }

        if let Some(runnable_task) = exec(|e| e.highest_priority_runnable_task()) {
            run_task(runnable_task);
            /* check for events which happened while task was running */
            exec(|e| e.epoll_wait(EpollTimeout::ZERO)).unwrap();
        } else {
            /* if there's no task to run, wait for an event */
            exec(|e| e.epoll_wait(EpollTimeout::NONE)).unwrap();
        }
    }
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
    exec(|e| e.shutdown());
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
