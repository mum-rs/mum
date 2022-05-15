# mum

Mumble daemon with controller (think `mpd(1)`/`mpc(1)`) written in Rust.

## Building (CURRENTLY OUT OF DATE)

**NOTE**: This is out of date as we are chaning the internal crate layout. It
will be fixed when the new layout is merged and we have time to update on crates
and the AUR.

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

### Installation

```sh
$ cargo install --path=mum
```

### Optional features

mum contains optional features that are enabled by default. To compile without
them, build with --no-default-features. Features can then be enabled separately with
--features "FEATURES".

The following features can be specified:

| Name              | Needed for         |
|-------------------|--------------------|
| mum/notifications | Notifications      |
| mum/ogg           | ogg sound effects  |

```sh
$ cargo build [--release] --no-default-features
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
