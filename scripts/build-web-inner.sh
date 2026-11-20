#!/bin/sh
# runs inside the web build image
set -eu
cd /build
webkit() {
  [ -f wpewebkit-$WPE_VER.tar.xz ] || curl -fLo wpewebkit-$WPE_VER.tar.xz https://wpewebkit.org/releases/wpewebkit-$WPE_VER.tar.xz
  echo "$WPE_SHA  wpewebkit-$WPE_VER.tar.xz" | sha256sum -c -
  [ -d wpewebkit-$WPE_VER ] || tar -xf wpewebkit-$WPE_VER.tar.xz
  # 2.54.3 bug: AcceleratedBackingStore.cpp uses DRM_FORMAT_XRGB8888 but includes drm_fourcc.h only with USE_LIBDRM; take the header only, no libdrm at runtime
  grep -q "libdrm/drm_fourcc.h" wpewebkit-$WPE_VER/Source/WebKit/UIProcess/wpe/AcceleratedBackingStore.cpp || perl -pi -e 's{^(#include <wpe/wpe-platform.h>)}{$1\n#include <libdrm/drm_fourcc.h>}' wpewebkit-$WPE_VER/Source/WebKit/UIProcess/wpe/AcceleratedBackingStore.cpp
  cmake -S wpewebkit-$WPE_VER -B wk -G Ninja -DPORT=WPE -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr -DCMAKE_INSTALL_LIBDIR=lib \
    -DUSE_LD_LLD=ON -DCMAKE_CXX_STANDARD_LIBRARIES=-lintl -DCMAKE_C_STANDARD_LIBRARIES=-lintl -DUSE_SKIA=ON -DENABLE_WPE_PLATFORM=ON -DENABLE_WPE_PLATFORM_HEADLESS=ON -DENABLE_WPE_PLATFORM_DRM=OFF -DENABLE_WPE_PLATFORM_WAYLAND=OFF \
    -DENABLE_WPE_LEGACY_API=OFF -DUSE_GSTREAMER=OFF -DENABLE_VIDEO=OFF -DENABLE_MEDIA_RECORDER=OFF -DENABLE_WEB_CODECS=OFF -DENABLE_WEB_AUDIO=OFF -DENABLE_MEDIA_STREAM=OFF -DENABLE_WEB_RTC=OFF -DENABLE_WEBGL=OFF \
    -DENABLE_BUBBLEWRAP_SANDBOX=OFF -DUSE_ATK=OFF -DENABLE_INTROSPECTION=OFF -DENABLE_DOCUMENTATION=OFF -DENABLE_WEBDRIVER=OFF \
    -DENABLE_SPEECH_SYNTHESIS=OFF -DENABLE_GAMEPAD=OFF -DENABLE_JOURNALD_LOG=OFF -DUSE_LIBBACKTRACE=OFF -DUSE_SYSPROF_CAPTURE=OFF \
    -DUSE_AVIF=OFF -DUSE_JPEGXL=OFF -DUSE_LCMS=OFF -DUSE_LIBHYPHEN=OFF -DUSE_GBM=OFF -DUSE_LIBDRM=OFF -DENABLE_WPE_QT_API=OFF \
    -DENABLE_MINIBROWSER=OFF -DENABLE_API_TESTS=OFF -DENABLE_PDFJS=OFF -DENABLE_SPELLCHECK=OFF
  ninja -C wk -j"$JOBS"
  DESTDIR=/build/wk-dest ninja -C wk install
}
ffmpeg() {
  [ -f ffmpeg-$FFMPEG_VER.tar.xz ] || curl -fLo ffmpeg-$FFMPEG_VER.tar.xz https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VER.tar.xz
  [ -d ffmpeg-$FFMPEG_VER ] || tar -xf ffmpeg-$FFMPEG_VER.tar.xz
  mkdir -p ff && cd ff
  ../ffmpeg-$FFMPEG_VER/configure --prefix=/usr --enable-shared --disable-static --disable-everything --disable-autodetect --enable-zlib --disable-network --disable-doc \
    --disable-debug --disable-avdevice --disable-swscale-alpha --disable-ffplay --disable-ffprobe \
    --enable-decoder=h264,vp8,vp9,aac,mp3,mp3float,opus,vorbis,flac,alac,pcm_s16le,pcm_s16be,pcm_s24le,pcm_s32le,pcm_f32le,pcm_u8,png,mjpeg,gif,webp,bmp \
    --enable-demuxer=mov,matroska,mp3,ogg,flac,wav,aac,image2pipe,gif,image_png_pipe,image_jpeg_pipe,image_webp_pipe,image_bmp_pipe \
    --enable-parser=h264,hevc,vp8,vp9,aac,mpegaudio,opus,vorbis,flac,png,mjpeg,webp,gif \
    --enable-filter=scale,fps,aresample,aformat,format,transpose,null,anull,volume \
    --enable-muxer=rawvideo,pcm_s16le,wav --enable-encoder=pcm_s16le,rawvideo --enable-protocol=file,pipe
  make -j"$JOBS"
  make DESTDIR=/build/ff-dest install
}
# The browser program (web/, needs WebKit's headers and libraries in /usr) and the sandbox shim (web/shim/, static), built from the repo mounted at /work. Not stamped: cargo does its own caching.
thirweb() {
  export CARGO_HOME=/build/cargo-home-web CFLAGS=-D_FILE_OFFSET_BITS=32 CC_aarch64_unknown_linux_musl=gcc   # CFLAGS: Lua's C source asks for fopen64 otherwise
  cp -a /build/wk-dest/usr/. /usr/   # the staged WebKit where the compiler looks (this container is thrown away)
  D=/build/thirweb-dest; rm -rf $D; mkdir -p $D/usr/bin $D/usr/libexec/wpe-webkit-2.0
  (cd /work/web && CARGO_TARGET_DIR=/build/thirweb-target cargo build --release)
  (cd /work/web/shim && RUSTFLAGS="-C target-feature=+crt-static" CARGO_TARGET_DIR=/build/thirweb-target-shim cargo build --release)
  cp /build/thirweb-target/release/thir-web $D/usr/bin/thir-web; cp /build/thirweb-target-shim/release/wpe-shim $D/usr/libexec/wpe-webkit-2.0/wpe-shim
  file $D/usr/libexec/wpe-webkit-2.0/wpe-shim | grep -qE "statically linked|static-pie" || { echo "wpe-shim is not static"; exit 1; }
  ls -l $D/usr/bin $D/usr/libexec/wpe-webkit-2.0
}
# stamps: each part has its own (version + the text of its function), so editing the ffmpeg flags never rebuilds WebKit and the other way round
stamp() { echo "$1 $(sed -n "/^$2() {/,/^}/p" "$0" | sha256sum | cut -d' ' -f1)"; }
cur() { [ -z "${FORCE:-}" ] && [ "$(cat "/build/.stamp-$1" 2>/dev/null)" = "$(stamp "$2" "$1")" ]; }
done_() { stamp "$2" "$1" > "/build/.stamp-$1"; }
if [ "$1" = all ]; then
  if cur ffmpeg $FFMPEG_VER; then echo "ffmpeg build is current"; else (ffmpeg); done_ ffmpeg $FFMPEG_VER; fi
  if cur webkit $WPE_VER; then echo "webkit build is current"; else (webkit); done_ webkit $WPE_VER; fi
else "$1"; fi
