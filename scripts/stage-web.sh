#!/bin/sh
# P0 spike: stage the web add-on closure (apk --root) from the thir-web-build volume and pack it as web.sqsh. Runs inside the web build image:
#   docker run --rm -v thir-web-build:/build -v $PWD/scripts:/scripts:ro thir-web-build sh /scripts/stage-web.sh
# The browser (P5) must run with the env in /web/etc/web.env (written below): WEBKIT_SKIA_ENABLE_CPU_RENDERING=1 LIBGL_ALWAYS_SOFTWARE=1 EGL_PLATFORM=surfaceless
# Result: /build/addon (tree), /build/${OUT_NAME:-web}.sqsh. Budget: BUDGET_MB (compressed).
set -eu
BUDGET_MB=${BUDGET_MB:-200}
# Mesa (llvmpipe) is part of the add-on for good: the web process needs an EGL display even with CPU rendering.
EXTRA_PKGS=${EXTRA_PKGS-mesa-egl mesa-gles mesa-dri-gallium mesa-gbm}
S=/build/stage; A=/build/addon
rm -rf $S $A; mkdir -p $S /build/apk-cache
apk add -q --root $S --initdb --cache-dir /build/apk-cache --no-scripts --keys-dir /etc/apk/keys --repositories-file /etc/apk/repositories \
  musl libstdc++ libgcc libatomic harfbuzz-icu glib libsoup3 glib-networking harfbuzz freetype fontconfig libpng libjpeg-turbo libwebp libwebpdemux libwebpmux \
  libxml2 libxslt libxkbcommon xkeyboard-config libepoxy icu-libs icu-data-en libgcrypt libtasn1 sqlite-libs ca-certificates-bundle \
  gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad alsa-lib font-noto font-noto-sinhala font-noto-tamil font-noto-arabic font-noto-hebrew font-noto-thai font-noto-devanagari font-droid-nonlatin brotli-libs zlib ${EXTRA_PKGS:-}
du -sk $S | awk '{printf "apk install tree: %.1f MB\n", $1/1024}'
cp -a /build/wk-dest/usr/. $S/usr/
# the browser (web/): thir-web, and the sandbox shim installed under the names of WebKit's helpers; the real programs are renamed .real and the shim runs them (web/shim)
cp /build/thirweb-dest/usr/bin/thir-web $S/usr/bin/
for p in WPEWebProcess WPENetworkProcess; do mv $S/usr/libexec/wpe-webkit-2.0/$p $S/usr/libexec/wpe-webkit-2.0/$p.real; cp /build/thirweb-dest/usr/libexec/wpe-webkit-2.0/wpe-shim $S/usr/libexec/wpe-webkit-2.0/$p; done
cp -a /build/ff-dest/usr/bin/ffmpeg $S/usr/bin/; cp -a /build/ff-dest/usr/lib/libav*.so* /build/ff-dest/usr/lib/libsw*.so* $S/usr/lib/
# GStreamer: only the plugins pages need (playback, MSE parsers, ALSA, the demuxers) plus gst-libav built against our ffmpeg; every other plugin is removed before the closure is computed
G=$S/usr/lib/gstreamer-1.0; cp /build/gl-dest/usr/lib/gstreamer-1.0/libgstlibav.so $G/
for f in $G/*.so; do case $(basename $f) in libgstcoreelements.so|libgsttypefindfunctions.so|libgstplayback.so|libgstapp.so|libgstaudioconvert.so|libgstaudioresample.so|libgstvideoconvertscale.so|libgstvideoconvert.so|libgstvideoscale.so|libgstvolume.so|libgstalsa.so|libgstopus.so|libgstvorbis.so|libgstogg.so|libgstisomp4.so|libgstmatroska.so|libgstaudioparsers.so|libgstautodetect.so|libgstid3demux.so|libgstvideoparsersbad.so|libgstlibav.so|libgstwavparse.so|libgstvideorate.so|libgstaudiorate.so|libgstopusparse.so|libgstdeinterlace.so|libgstaudiofx.so|libgstvideofilter.so|libgstsubenc.so) ;; *) rm -f $f;; esac; done
ls $G | tr '\n' ' '; echo
# keep only what the ELF files need (NEEDED closure from the WebKit/ffmpeg/gio roots); copy it to $A
python3 - <<'PY'
import os, subprocess, shutil
S, A = "/build/stage", "/build/addon"
libdirs = [S+"/usr/lib", S+"/lib"]
def needed(p):
    out = subprocess.run(["readelf", "-d", p], capture_output=True, text=True).stdout
    return [l.split("[")[1].split("]")[0] for l in out.splitlines() if "(NEEDED)" in l]
def find(n):
    for d in libdirs:
        q = os.path.join(d, n)
        if os.path.exists(q): return os.path.realpath(q)
def is_elf(p): return os.path.isfile(p) and not os.path.islink(p) and open(p, "rb").read(4) == b"\x7fELF"
roots = []
for d, _, fs in os.walk(S + "/usr"):
    for f in fs:
        p = os.path.join(d, f)
        if is_elf(p) and ("wpe-webkit" in p or "gio/modules" in p or "libWPE" in f or "libjavascript" in f or f.startswith(("libavcodec", "libavformat", "libavutil", "libavfilter", "libswscale", "libswresample")) or "gstreamer-1.0" in p or f in ("ffmpeg", "thir-web") or "/dri/" in p or f.startswith(("libEGL", "libGLESv2", "libgbm", "libglapi"))): roots.append(p)
keep, todo, missing = set(roots), list(roots), set()
while todo:
    p = todo.pop()
    for n in needed(p):
        q = find(n)
        if q is None: missing.add(n); continue
        if q not in keep: keep.add(q); todo.append(q)
print("roots", len(roots), "libs kept", len(keep), "unresolved", sorted(missing))
for lib in sorted(keep):
    rel = os.path.relpath(lib, S); dst = os.path.join(A, rel); os.makedirs(os.path.dirname(dst), exist_ok=True); shutil.copy2(lib, dst)
open("/build/addon-keep.txt", "w").write("\n".join(sorted(os.path.relpath(k, S) for k in keep)))
PY
# soname links for every kept lib, the musl loader, data
cd $S
for f in $(cat /build/addon-keep.txt); do d=$(dirname $f); for l in $(ls $d); do [ -L "$S/$d/$l" ] && [ "$(readlink -f $S/$d/$l)" = "$S/$f" ] && { mkdir -p $A/$d; cp -a $S/$d/$l $A/$d/; }; done; done
mkdir -p $A/lib $A/usr/lib $A/etc $A/usr/share
cp -a $S/lib/ld-musl-* $A/lib/ 2>/dev/null || true
cp -a $S/usr/libexec $A/usr/ 2>/dev/null || true; rm -rf $A/usr/libexec/gstreamer-1.0   # no scanner process: the registry is made in-process
cp -a $S/usr/lib/gio $A/usr/lib/ 2>/dev/null || true; cp -a $S/usr/lib/gstreamer-1.0 $A/usr/lib/
mkdir -p $A/usr/share/alsa; cp $S/usr/share/alsa/alsa.conf $A/usr/share/alsa/; cp -a $S/usr/share/alsa/alsa.conf.d $A/usr/share/alsa/ 2>/dev/null || true; cp -a $S/usr/share/alsa/cards $S/usr/share/alsa/pcm $A/usr/share/alsa/ 2>/dev/null || true
printf 'pcm.!default { type hw card 0 device 0 subdevice 0 }\nctl.!default { type hw card 0 }\n' >> $A/usr/share/alsa/alsa.conf   # the one sound card, directly: GStreamer converts the rate and format (VirtualBox HDA plays 44.1 kHz at most)
[ -z "${EXTRA_PKGS:-}" ] || { cp -a $S/usr/lib/dri $A/usr/lib/ 2>/dev/null || true; cp -a $S/usr/share/drirc.d $A/usr/share/ 2>/dev/null || true; }
cp -a $S/usr/lib/libicudata* $A/usr/lib/; cp -a $S/usr/share/icu $A/usr/share/; rm -f $A/usr/lib/gio/modules/libgiolibproxy.so $A/usr/lib/gio/modules/libgiognomeproxy.so
cp -a $S/etc/fonts $A/etc/; cp -a $S/etc/ssl $A/etc/ 2>/dev/null || true
mkdir -p $A/usr/bin; cp -a $S/usr/bin/ffmpeg $S/usr/bin/thir-web $A/usr/bin/
mkdir -p $A/usr/share/fonts/noto
for f in NotoSans-Regular NotoSans-Bold NotoSans-Italic NotoSans-BoldItalic NotoSerif-Regular NotoSerif-Bold NotoSansMono-Regular NotoSansMono-Bold NotoSansSinhala-Regular NotoSansSinhala-Bold NotoSansTamil-Regular NotoSansTamil-Bold NotoSansArabic-Regular NotoSansArabic-Bold NotoSansHebrew-Regular NotoSansHebrew-Bold NotoSansThai-Regular NotoSansThai-Bold NotoSansDevanagari-Regular NotoSansDevanagari-Bold; do cp $S/usr/share/fonts/noto/$f.ttf $A/usr/share/fonts/noto/ 2>/dev/null || echo "no font $f"; done
# CJK (and the rest) fallback: Droid Sans Fallback, 3.9 MB, Apache-2.0
mkdir -p $A/usr/share/fonts/droid; cp $S/usr/share/fonts/droid-nonlatin/DroidSansFallback.ttf $A/usr/share/fonts/droid/
# fontconfig: only the conf.d snippets that make sense for these fonts
rm -f $A/etc/fonts/conf.d/*noto-math* $A/etc/fonts/conf.d/*noto-symbols* $A/etc/fonts/conf.d/*unifont* $A/etc/fonts/conf.d/*persian* $A/etc/fonts/conf.d/*delicious*
# fontconfig cache made now, read-only at run time: the browser's helpers cannot write ("No writable cache directories"). Paths in it are those inside the image (/usr/share/fonts).
sed -i 's#<cachedir>/var/cache/fontconfig</cachedir>#<cachedir>/usr/share/fontconfig-cache</cachedir>#' $A/etc/fonts/fonts.conf; grep -q fontconfig-cache $A/etc/fonts/fonts.conf
mkdir -p $A/usr/share/fontconfig-cache; fc-cache -f -y $A; ls $A/usr/share/fontconfig-cache | head -3
# xkb: us/pc/evdev only
X=$S/usr/share/X11/xkb; mkdir -p $A/usr/share/X11/xkb
for d in keycodes types compat rules; do cp -a $X/$d $A/usr/share/X11/xkb/; done
mkdir -p $A/usr/share/X11/xkb/symbols; cp $X/symbols/us $X/symbols/pc $X/symbols/inet $X/symbols/level3 $X/symbols/keypad $X/symbols/altwin $X/symbols/ctrl $X/symbols/capslock $X/symbols/shift $X/symbols/nbsp $X/symbols/srvr_ctrl $X/symbols/compose $X/symbols/group $X/symbols/terminate $A/usr/share/X11/xkb/symbols/ 2>/dev/null || true
rm -rf $A/usr/share/X11/xkb/rules/*.xml $A/usr/share/X11/xkb/rules/*.lst 2>/dev/null || true
mkdir -p $A/usr/share/wpe-webkit-2.0; cp -a $S/usr/share/wpe-webkit-2.0/. $A/usr/share/wpe-webkit-2.0/ 2>/dev/null || true
rm -rf $A/usr/lib/pkgconfig $A/usr/include $A/usr/lib/cmake
printf 'WEBKIT_SKIA_ENABLE_CPU_RENDERING=1\nLIBGL_ALWAYS_SOFTWARE=1\nEGL_PLATFORM=surfaceless\nMESA_DEBUG=silent\nLIBGL_DEBUG=quiet\nEGL_LOG_LEVEL=fatal\n' > $A/etc/web.env
# strip
find $A -type f \( -name '*.so*' -o -perm -u+x \) | while read -r f; do [ "$(head -c4 "$f" | tr -d '\0')" = "$(printf '\177ELF')" ] && strip --strip-unneeded "$f" 2>/dev/null || true; done
du -sk $A | awk '{printf "addon tree (stripped): %.1f MB\n", $1/1024}'
for d in usr/lib/gstreamer-1.0 usr/lib/libWPEWebKit* usr/lib/libWPEPlatform* usr/lib/libicudata* usr/libexec usr/bin usr/share/fonts usr/share/X11 etc usr/lib/libav*; do du -sk $A/$d 2>/dev/null | awk -v n=$d '{printf "  %-34s %7.1f MB\n", n, $1/1024}'; done
rm -f /build/${OUT_NAME:-web}.sqsh; mksquashfs $A /build/${OUT_NAME:-web}.sqsh -comp zstd -Xcompression-level 19 -all-root -noappend -quiet
SZ=$(stat -c %s /build/${OUT_NAME:-web}.sqsh); echo "${OUT_NAME:-web}.sqsh: $((SZ/1024/1024)) MB ($SZ bytes), budget ${BUDGET_MB} MB"
[ "$SZ" -le $((BUDGET_MB*1024*1024)) ] || { echo "OVER BUDGET"; exit 1; }
