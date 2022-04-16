#!/usr/bin/env sh
PKG_CONFIG_ALLOW_CROSS=1 cargo build --target x86_64-pc-windows-gnu --no-default-features --bin mumrepl
