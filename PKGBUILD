# Maintainer: Eskil Queseth <eskilq at kth dot se>
# Maintainer: Gustav Sörnäs <gustav at sornas dot net>

pkgname=mum-git
pkgver=0.1.0
pkgrel=1
pkgdesc="A mumble client/daemon pair"
arch=('x86_64')
url="https://github.com/sornas/mum.git"
license=('MIT')
sha256sums=('SKIP')
depends=('alsa-lib' 'opus' 'openssl')
optdepends=('bash: for tab-completions',
            'fish: for tab-completions',
            'zsh: for tab-completions')
makedepends=('git' 'rust')
source=("git+$url")

build() {
    cd "${srcdir}/${pkgname%-git}"

    cargo build --release --target-dir=target

    which bash &>/dev/null && ./target/release/mumctl completions --bash > mumctl.bash
    which fish &>/dev/null && ./target/release/mumctl completions --fish > mumctl.fish
    which zsh &>/dev/null && ./target/release/mumctl completions --zsh > mumctl.zsh
}

check() {
    cd "${srcdir}/${pkgname%-git}"
    cargo test --release --target-dir=target
}

package() {
    cd "${srcdir}/${pkgname%-git}"

    which bash &>/dev/null && install -Dm 755 mumctl.bash "${pkgdir}/usr/share/bash-completion/completions/mumctl"
    which fish &>/dev/null && install -Dm 755 mumctl.fish "${pkgdir}/usr/share/fish/completions/mumctl.fish"
    which zsh &>/dev/null && install -Dm 755 mumctl.zsh "${pkgdir}/usr/share/zsh/site-functions/_mumctl"

    install -Dm 755 target/release/mumctl -t "${pkgdir}/usr/bin"
    install -Dm 755 target/release/mumd -t "${pkgdir}/usr/bin"
    install -Dm 644 LICENSE -t "${pkgdir}/usr/share/licenses/${pkgname}"
}
