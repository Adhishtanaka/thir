# Build image for the web add-on (WPE WebKit + minimal ffmpeg). Alpine is pinned: the add-on ships this release's libraries.
FROM alpine:3.24
RUN apk add --no-cache build-base cmake ninja perl python3 ruby gperf unifdef git curl xz tar pkgconf linux-headers lld \
    squashfs-tools strace file binutils nasm \
    glib-dev libsoup3-dev libxml2-dev libxslt-dev sqlite-dev icu-dev harfbuzz-dev freetype-dev fontconfig-dev \
    libjpeg-turbo-dev libpng-dev libwebp-dev woff2-dev libgcrypt-dev libtasn1-dev libxkbcommon-dev libepoxy-dev \
    zlib-dev brotli-dev libpsl-dev gettext-dev libdrm-dev rust cargo
