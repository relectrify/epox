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
    cell::RefCell,
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
    /* write the task's address to this pipe to wake the task */
    pipe_wr: RawFd,
    /* use this waker to wake the task */
    waker: Waker,
    pub(crate) task: Pin<Box<RefCell<Task>>>,
}
pub(crate) type TaskRef = Pin<Arc<ExecutorTask>>;

impl Queueable for ExecutorTask {
    fn with_entry<R, F: FnMut(Pin<&mut QueueEntry>) -> R>(self: Pin<&Self>, mut f: F) -> R {
        let mut s = self.task.borrow_mut();
        /* safety: self is pinned therefore borrowed self is also pinned */
        let s = unsafe { Pin::new_unchecked(&mut *s) };
        f(s.queue_entry())
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
        let task = Pin::new(Arc::new_cyclic(|me| ExecutorTask {
            pipe_wr: self.pipe_wr.as_raw_fd(),
            waker: build_task_waker(me),
            task: Task::new_pinned_boxed_refcell(future, priority),
        }));
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

    fn epoll_wait(mut self: Pin<&mut Self>) -> Result<(), Error> {
        if self.events.is_empty() {
            return Ok(());
        }
        let n_events = {
            let this = self.as_mut().project();
            this.epoll
                .wait(this.events.as_mut_slice(), EpollTimeout::NONE)
        }?;
        let mut check_pipe = false;
        /* wakeup futures which have an event, adding them to runq */
        for ei in 0..n_events {
            let e = self.events[ei];
            if e.data() == 0 {
                check_pipe = true;
                continue;
            }
            /* safety: e.data() is the result of std::ptr::from_ref(Pin::into_inner) */
            let ew = Pin::new(unsafe { &*(e.data() as *mut RefCell<EpollWaker>) });
            let mut waker = ew.borrow_mut();
            waker.events |= e.events();
            /* waker.waker may be a noop waker - in that case, waker.waker.data() will
             * not be a valid pointer */
            if !waker.waker.data().is_null() {
                /* don't call waker.waker.wake_by_ref() as the executor is already borrowed! */
                let weak = weak_from_raw(waker.waker.data());
                if let Some(p) = weak.upgrade() {
                    self.as_mut().enqueue(Pin::new(p));
                }
                let _ = Weak::into_raw(weak);
            }
        }

        if check_pipe {
            self.as_mut().wake_from_pipe()?;
        }

        Ok(())
    }

    fn enqueue(self: Pin<&mut Self>, task: TaskRef) {
        let priority = task.task.borrow().priority();
        self.runq(priority).push(task);
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
            self.as_mut().enqueue(Pin::new(task));
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
fn build_task_waker(task: &Weak<ExecutorTask>) -> Waker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake_by_val, wake_by_ref, drop);

    fn wake(weak: &Weak<ExecutorTask>) {
        if let Some(task) = weak.upgrade() {
            exec(|e| {
                // Waker implements Send + Sync, so this needs to wake the task from any thread.
                // Each executor listens for tasks on a pipe, which the task stores in pipe_wr.
                // We send the task pointer into this pipe where the corresponding thread will
                // see it and enqueue the task.

                // If we're already on the same thread we don't need to go through the kernel -
                // we can directly enqueue the task.

                // However, enqueue will need to mutably borrow the task. So if the task is
                // already borrowed, send it into the pipe - we know no tasks are borrowed when
                // wake_from_pipe is called from epoll_wait

                // The task will already be borrowed when it wakes itself, which it is
                // explicitly permitted to do according to the waker specification.

                // See https://doc.rust-lang.org/std/task/struct.Waker.html#method.wake

                // So if the task isn't already borrowed _and_ we're on the same thread as the
                // task, we enqueue the task directly without going through the pipe.
                if task.pipe_wr == e.pipe_wr.as_raw_fd() && task.task.try_borrow_mut().is_ok() {
                    e.enqueue(Pin::new(task));
                } else {
                    let p = Weak::into_raw(weak.clone());
                    let bytes = (p as usize).to_ne_bytes();
                    /* safety: fd stays valid for duration of call */
                    write(unsafe { BorrowedFd::borrow_raw(task.pipe_wr) }, &bytes).unwrap();
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

    unsafe { Waker::from_raw(RawWaker::new(Weak::into_raw(task.clone()).cast(), &VTABLE)) }
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
     * Handle events on this waker. Caller must return the result of the io
     * (usually read() or write()) operation which was run on the associated
     * file descriptor.
     */
    pub(crate) fn poll_with<T, F: FnMut(EpollFlags) -> Result<T, std::io::Error>>(
        &mut self,
        cx: &std::task::Context<'_>,
        mut func: F,
    ) -> Poll<Result<T, std::io::Error>> {
        if self.events.is_empty() {
            self.waker.clone_from(cx.waker());
            return Poll::Pending;
        }
        match func(self.events) {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                self.waker.clone_from(cx.waker());
                self.events = EpollFlags::empty();
                Poll::Pending
            }
            res => Poll::Ready(res),
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

/**
 * Run the thread local executor until no futures are pending.
 */
pub fn run() -> Result<(), std::io::Error> {
    'outer: loop {
        while let Some(t) = exec(|e| e.highest_priority_runnable_task()) {
            let pending = {
                /* poll the task */
                let mut cx = Context::from_waker(&t.waker);
                let mut t = t.task.borrow_mut();
                /* safety: t is pinned therefore its deref is also pinned */
                let t = unsafe { Pin::new_unchecked(&mut *t) };
                t.poll(&mut cx) == Poll::Pending
            };

            if exec(|e| e.shutdown) {
                break 'outer;
            }

            if pending {
                /* task pending: put it on the sleep queue */
                exec(|e| e.sleep(t));
            }
        }
        if exec(|e| e.sleepq.is_empty()) {
            /* all tasks have completed */
            break;
        }

        /* wait for events */
        exec(|e| e.epoll_wait())?;
    }

    // drop the executor, releasing all resources it has used
    // replaces it with a new one so you can call run() again
    let _ = EXECUTOR.replace(Executor::new()?);

    Ok(())
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
