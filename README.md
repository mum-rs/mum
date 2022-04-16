# mum

A small Mumble client split into frontend and backend written in Rust.

Two different frontends are included for different needs and wants.

- Daemon and controller pair (think `mpd(1)`/`mpc(1)`).
- REPL.

The daemon/controller-pair only runs on Unix due to Unix-sockets. The REPL runs
on both Unix and Windows and works more like what you expect from mainstream
Mumble clients.

The backend is event-driven and pretty 1:1 with the CLI. The goal is for it to
be able to function as a bot library but we're not fully there yet. Stay tuned!

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

- rust (stable, no guaranteed minimum version)
- alsa-lib
- openssl
- opus
- libnotify (optional, needed in default configuration)

### Installation

```sh
$ cargo install --path=mum
```

### Optional features

mum contains optional features that are enabled by default. To compile without
them, build/install with --no-default-features. Features can then be enabled separately with
--features "FEATURES".

The following features can be specified:

| Name              | Needed for         |
|-------------------|--------------------|
| mum/notifications | Notifications      |
| mum/ogg           | ogg sound effects  |

### man-pages

Man-pages for mumd, mumctl and mumdrc (the configuration file) are included as
both asciidoc txt-files and already formatted groff-files. They are generated
with

```sh
$ asciidoctor -b manpage *.txt
```

## Usage (Daemon/controller)

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
...
$ mumctl channel list
ServerRoot
  -user1
  -user2
  -user2
  Channel2
  Channel3
$ mumctl channel connect Channel2
```

## Usage (REPL)

This describes how to connect to a server and join different channels.

```sh
$ mumrepl
>> server connect 127.0.0.1 spock
...
>> channel list
ServerRoot
  -user1
  -user2
  -user2
  Channel2
  Channel3
>> channel connect Channel2
```

## Known issues

The main hub for issues is [our issue
tracker](https://github.com/mum-rs/mum/issues).

## Why?

Mostly because it's a fun way of learning a new language. Also:

- Most mumble clients use a GUI. While GUIs aren't necessarily bad, there
  should at least exist alternatives where possible.
- Memory, disk and CPU usage. We haven't found a reliable way of testing this
  yet (suggestions welcome).

## Other projects

- [Barnard (go)](https://github.com/bmmcginty/barnard.git) - TUI mumble client
