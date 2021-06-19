# mum

Mumble daemon with controller (think `mpd(1)`/`mpc(1)`) written in Rust.

## Building

mumd and mumctl are available on crates.io and can be installed with

```sh
$ cargo install mumd
$ cargo install mumctl
```

They are also
[available on the AUR](https://aur.archlinux.org/packages/mum-git/). Thirdly, we
publish [compiled binaries on Github](https://github.com/sornas/mum/releases/).

### Requirements

These are for Arch Linux. You might need other packages on other distributions
and operating systems, or they might be named something different.

- rust (stable)
- alsa-lib
- openssl
- opus
- libnotify (optional, needed in default configuration)

Windows is not currently supported but could be in the future. macOS should work.
Other operating systems haven't been tested. The limiting factor on Windows
is IPC communication which is (currently) done via the crate ipc-channel.

We only "guarantee" compilation on latest Rust stable. Open a ticket if this is
an issue for you and we'll see what we can do.

### Installation

1. Build the binaries
2. (wait)
3. Copy/symlink to somewhere nice (or don't).

```sh
$ cargo build --release
$ ln -s $PWD/target/release/mumctl $HOME/.local/bin/
$ ln -s $PWD/target/release/mumd $HOME/.local/bin/
```

### Optional features

mum contains optional features that are enabled by default. To compile without
them, build with --no-default-features. Features can then be enabled with
--features "FEATURES".

The following features can be specified:

| Name               | Needed for    |
|--------------------|---------------|
| mumd/notifications | Notifications |

If you're using Cargo 1.51 or later you can specify features directly from the
workspace root:

```sh
$ cargo build [--release] --no-default-features
```

Older versions need to build the packages separately:

```sh
$ cd mumd
$ cargo build --release --no-default-features
$ cd ../mumctl
$ cargo build --release --no-default-features  # technically unneeded
                                               # since no features exist
```

### man-pages

Man-pages for mumd, mumctl and mumdrc (the configuration file) are included as
both asciidoc txt-files and already formatted groff-files. They are generated
with

```sh
$ asciidoctor -b manpage *.txt
```

## Usage

This describes how to connect to a server and join different channels.
See `$ mumctl --help` or `documentation/*.txt` for more information.

### mumd

Start the daemon with mumd. Currently it attaches to the terminal, so if you
want to run it in the background you can detach it with e.g. (zsh): 

```sh
$ mumd &>/dev/null &|
```

Somewhere down the line we're aiming to have a `--daemonize`.

### mumctl

Interfacing with the daemon is done through mumctl. Some examples:

```sh
$ mumctl connect 127.0.0.1 spock # connect to 127.0.0.1 with username 'spock'
$ mumctl channel list
ServerRoot
  -user1
  -user2
  -user2
  Channel2
  Channel3
$ mumctl channel connect Channel2
```

## Known issues

The main hub for issues is [our issue
tracker](https://github.com/mum-rs/mum/issues). Additionally, there are some
features that aren't present on the issue tracker:

- Administration tools. See [the admin tools
  project](https://github.com/mum-rs/mum/projects/1).
- Surround output. If this is something you want, [open an
  issue](https://github.com/mum-rs/mum/issues/new) so we can take a look at
  implementing it.

## Why?

Mostly because it's a fun way of learning a new language. Also:

- Most mumble clients use a GUI. While GUIs aren't necessarily bad, there
  should at least exist alternatives where possible.
- Memory, disk and CPU usage. We haven't found a reliable way of testing this
  yet (suggestions welcome).

## Other projects

- [Barnard (go)](https://github.com/bmmcginty/barnard.git) - TUI mumble client
