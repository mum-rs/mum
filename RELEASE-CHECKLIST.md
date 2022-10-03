# Preparation

- Create a new branch `x.y` and base all commits on it.

# Getting the final binary

- Run `$ cargo update`.
- Check `$ cargo outdated`.
- Build final version:
  - `$ MUM_VERSION=x.y.z RUSTFLAGS="--remap-path-prefix=$(pwd)=" cargo build --release`
  - `$ cp target/release/mum{ctl,d}`
- Basic test:
  - Check `--version`.
  - Connect to server.
  - Connect with official mumble client.
  - Mute mumd and check if sound can be received.
  - Mute mumble and check if sound can be sent.
  - Check `$ mumctl status`.
  - Send a text message.
  - Receive a text message.

# Publish to Github

- Set the version header and today's date in the changelog.
- Create a new "Unreleased" header.
- Final commits:
  - Cargo.lock and Cargo.toml.
  - Updated changelog.
- Create a tag: `$ git tag vx.y.z`.
- Merge into main: `$ git switch main && git merge --no-ff x.y`.
- Push both branches and the tag.
- Create a new release on Github targeting the pushed tag.
  - Copy the changelog (change headers to `##`-headers).
  - Copy the output of `$ git diff va.b.c..vx.y.z --stat=80` where a.b.c is the
    previously released version.

# Publish to crates.io

Note that there might be a delay where crates.io catches up to the updated
repository.

- `$ (cd mumlib && cargo publish)`
- `$ (cd mum && cargo publish)`
