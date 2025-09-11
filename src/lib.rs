//! # Features
//!
//! `nightly`: enable features that require a nightly toolchain

#![cfg_attr(feature = "nightly", feature(never_type))]

pub mod signal;
pub mod timer;

pub use executor::{Priority, run, shutdown, spawn, spawn_with_priority};
pub use fd::Fd;
pub use nix::sys::epoll::EpollFlags;
pub use signal::AsyncSignal;
pub use task::yield_now;
pub use timer::Timer;

/** Compatibility with futures-rs. */
pub mod futures;

pub mod executor;
pub mod fd;
pub mod task;
