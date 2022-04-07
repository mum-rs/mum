# mum

Mumble daemon with controller (think `mpd(1)`/`mpc(1)`) written in Rust.

## Building

mumd and mumctl are available on crates.io and can be installed with

```sh
$ cargo install mum
```

You can also install the latest version from Github directly via cargo:

```sh
$ cargo install mum --git=https://github.com/mum-rs/mum
```

There is also a package [available on the
AUR](https://aur.archlinux.org/packages/mum-git/), and we sometimes publish
[compiled binaries on Github](https://github.com/mum-rs/mum/releases/).

### Requirements

These are for Arch Linux. You might need other packages on other distributions
and operating systems, or they might be named something different.

- rust (stable 1.53)
- alsa-lib
- openssl
- opus
- libnotify (optional, needed in default configuration)

Windows is not currently supported but could be in the future. macOS should work.
Other operating systems haven't been tested. The limiting factor on Windows
is IPC communication which is (currently) done via the crate ipc-channel.

We only "guarantee" compilation on latest Rust stable. Open a ticket if this is
an issue for you and we'll see what we can do.

### Optional features

mum contains optional features that are enabled by default. To compile without
them, build with --no-default-features. Features can then be enabled with
--features "FEATURES".

The following features can be specified:

| Name          | Needed for         |
|---------------|--------------------|
| notifications | Notifications      |
| ogg           | ogg sound effects  |

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

## Bugs and new features

The main hub for issues is [our issue
tracker](https://github.com/mum-rs/mum/issues). The development rate comes and
goes but at least one developer uses it as a daily driver.

## Why?

Mostly because it's a fun way of learning a new language. Also:

- Most mumble clients use a GUI. While GUIs aren't necessarily bad, there
  should at least exist alternatives where possible.
- Memory, disk and CPU usage. We haven't found a reliable way of testing this
  yet (suggestions welcome).

## Other projects

- [Barnard (go)](https://github.com/bmmcginty/barnard.git) - TUI mumble client
