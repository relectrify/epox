`epox`
======

`epox` is an async executor for Rust based on the Linux epoll I/O event
notification facility.

`epox` differs from other async executors in the Rust ecosystem as it:
* Attempts to minimise latency from event notification to task execution.
* Provides a priority system so that I/O events can be ordered.
* Does not spawn threads for task execution or move tasks between threads.
* Contains as little code as possible.
* Tries to be simple and easy to understand.
* Only runs on Linux.

We aim to provide a minimal executor which minimises I/O latency as much as
possible while providing a high degree of control to application authors.
`epox` is well suited for applications such as real-time control, audio
processing, games and other applications where it is important to separate time
critical tasks from other processing.

The `epox` async executor is thread-local. This means that:
* `epox::spawn()` spawns tasks on the thread-local executor only.
* `epox::spawn()` can be called from any context, async or not.
* Tasks do not move between threads.
* Data which is shared between tasks running on the same thread-local executor
  can be stored in an `Rc<RefCell>` rather than an `Arc<Mutex>`.

Even though the `epox` async executor is thread-local, threading is fully
supported. Calling `epox::spawn()` from two different threads runs tasks in
parallel in two completely separate executors.

See the `examples/` directory to get started.
