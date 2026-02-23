0.2.2
=====

- Fix `epox::block_on()` hang if the future uses `epox::thread::spawn()`

0.2.1
=====

- Teach `AsyncWriteFd::poll_flush()` to support more fd types (e.g. sockets).

0.2.0
=====

- Expose `epox::executor::block_on()` as `epox::block_on()`
- Change `epox::block_on()` to work with unpinned future.

0.1.1
=====

- More documentation.
- Add `epox::executor::block_on()`.

0.1.0
=====

- Initial release.
