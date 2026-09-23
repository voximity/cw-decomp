# cw-decomp

Decompilation of Cube World Alpha, the July 23, 2013 build, and its port to Rust with a
Vulkan renderer for x86_64 Windows, x86_64 Linux, and aarch64 macOS.

Requires a copy of the game to function.

## AI use notice

This project was almost entirely built with the use of LLMs. As a result, it's licensed
under the [PolyForm Noncommercial License 1.0.0](LICENSE): free to use, study, and modify
for any noncommercial purpose, but not licensed for commercial use.

The code quality is frankly terrible. It looks like what it is: ported pseudocode. Your
LLM should still be able to make sense of what is happening where if you want to change
things about the game or learn about how Cube World works (with one hideous layer of indirection).

## Compatibility

This project aims to be a very faithful port of the last Alpha build, with many known
quirks and problems being ported straight in.

You can expect it to play identically to the alpha build but with fewer rendering or
software gimmicks. It also targets Linux and macOS (via Metal) for better platform support.

As a result, a `cw-client` can be played against the source `Server.exe` or vice versa,
a `Cube.exe` client connecting to a `cw-server` instance.

## Usage

To run either the client or the server, obtain their executable:

```
cargo build -p cw-client --release
cargo build -p cw-server --release
```

Then, create `game/` in the working directory where you'll run the game and paste in
the original alpha build from July 23, 2013. The port uses the same asset files from
the original. Alternatively, specify `CW_GAME_DIR` to point at where you have the
alpha installed.

Then, simply run either executable. To change the port or seed on the server, use the
`--port` and `--seed` flags passed to the server executable.

## Future goals

This project, `cw-decomp`, is one of two of a larger Cube World-related undertaking.
This project will remain a faithful port of the alpha build to Rust, complete with all
of its quirks, but work is underway on a fork of this project with a modernized engine,
many bug fixes and balancing changes, and more game features. Essentially a spiritual
continuation of the original Cube World that my 10 year old self would have enjoyed.

## License

This project is licensed under [PolyForm Noncommercial License 1.0.0](LICENSE).
It must not be used in any commercial contexts.
