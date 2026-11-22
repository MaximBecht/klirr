# Local test audio

Put your own recordings here, e.g. a dry guitar recording (DI, straight from the interface)
to run through the `render` example:

```sh
cargo run --release --example render -- audio/di.wav audio/out.wav channel=2 gain=0.7 cabinet=2
```

Everything in this folder except this README is ignored by git, so recordings and renders
never end up in the repository.
