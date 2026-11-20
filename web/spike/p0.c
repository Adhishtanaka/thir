// thir P0 spike: custom WPEPlatform display/view/toplevel, load js.html, write the first WPEBufferSHM frame as PNG.
// build: cc p0.c -o p0 $(pkg-config --cflags --libs wpe-webkit-2.0 wpe-platform-2.0 libpng)
#include <wpe/webkit.h>
#include <wpe/wpe-platform.h>
#include <png.h>
#include <string.h>
#include <stdio.h>
#include <stdlib.h>

#define W 800
#define H 600
static GMainLoop *loop; static const char *out_png; static int exit_code = 1;
static WPEBuffer *cur, *prev; static gboolean saw_shm, title_ok; static WPEView *theview;

/* --- toplevel: no window, just tracks size --- */
typedef struct { WPEToplevel p; } ThirToplevel; typedef struct { WPEToplevelClass p; } ThirToplevelClass;
G_DEFINE_TYPE(ThirToplevel, thir_toplevel, WPE_TYPE_TOPLEVEL)
static gboolean tl_resize(WPEToplevel *t, int w, int h) { wpe_toplevel_resized(t, w, h); return TRUE; }
static void thir_toplevel_init(ThirToplevel *t) {}
static void thir_toplevel_class_init(ThirToplevelClass *k) { WPE_TOPLEVEL_CLASS(k)->resize = tl_resize; }

/* --- view: receives the buffers --- */
typedef struct { WPEView p; guint idle; } ThirView; typedef struct { WPEViewClass p; } ThirViewClass;
G_DEFINE_TYPE(ThirView, thir_view, WPE_TYPE_VIEW)
static void thir_view_init(ThirView *v) {}
static gboolean frame_done(gpointer d) {   /* pretend the frame hit the screen */
    WPEView *v = d; ((ThirView *)v)->idle = 0;
    if (prev) { wpe_view_buffer_released(v, prev); g_clear_object(&prev); }
    prev = cur; cur = NULL;      /* keep the shown buffer until the next one arrives */
    if (prev) wpe_view_buffer_rendered(v, prev);
    return G_SOURCE_REMOVE;
}
static gboolean view_render(WPEView *v, WPEBuffer *b, const WPERectangle *d, guint n, GError **e) {
    if (WPE_IS_BUFFER_SHM(b)) saw_shm = TRUE;
    printf("render_buffer %s %dx%d damage=%u\n", G_OBJECT_TYPE_NAME(b), wpe_buffer_get_width(b), wpe_buffer_get_height(b), n); fflush(stdout);
    g_clear_object(&cur); cur = g_object_ref(b);
    ThirView *tv = (ThirView *)v; if (!tv->idle) tv->idle = g_idle_add(frame_done, v);
    return TRUE;
}
static void thir_view_class_init(ThirViewClass *k) { WPE_VIEW_CLASS(k)->render_buffer = view_render; }

/* --- display: no EGL, no DRM, no input --- */
typedef struct { WPEDisplay p; } ThirDisplay; typedef struct { WPEDisplayClass p; } ThirDisplayClass;
G_DEFINE_TYPE(ThirDisplay, thir_display, WPE_TYPE_DISPLAY)
static void thir_display_init(ThirDisplay *d) {}
static gboolean d_connect(WPEDisplay *d, GError **e) { return TRUE; }
static WPEView *d_create_view(WPEDisplay *d) { return g_object_new(thir_view_get_type(), "display", d, NULL); }
static WPEToplevel *d_create_toplevel(WPEDisplay *d, guint m) { return g_object_new(thir_toplevel_get_type(), "display", d, NULL); }
static void thir_display_class_init(ThirDisplayClass *k) {
    WPEDisplayClass *c = WPE_DISPLAY_CLASS(k); c->connect = d_connect; c->create_view = d_create_view; c->create_toplevel = d_create_toplevel;
}

static int save_png(WPEBufferSHM *b, int w, int h) {
    gsize len; const guchar *px = g_bytes_get_data(wpe_buffer_shm_get_data(b), &len); guint stride = wpe_buffer_shm_get_stride(b);
    printf("frame %dx%d stride=%u bytes=%zu centre pixel BGRA=%02x%02x%02x%02x\n", w, h, stride, len,
        px[(h/2)*stride + (w/2)*4], px[(h/2)*stride + (w/2)*4+1], px[(h/2)*stride + (w/2)*4+2], px[(h/2)*stride + (w/2)*4+3]);
    png_image im; memset(&im, 0, sizeof im); im.version = PNG_IMAGE_VERSION; im.width = w; im.height = h; im.format = PNG_FORMAT_BGRA;
    int ok = png_image_write_to_file(&im, out_png, 0, px, stride, NULL);
    int r = px[(h/2)*stride + (w/2)*4+2], g = px[(h/2)*stride + (w/2)*4+1], bl = px[(h/2)*stride + (w/2)*4];
    return ok && r == 2 && g == 200 && bl == 150;   /* page colour set by JS */
}
static gboolean snap(gpointer d) {
    WPEBuffer *b = prev ? prev : cur;
    if (!b || !WPE_IS_BUFFER_SHM(b)) { puts("no SHM buffer"); exit_code = 2; }
    else exit_code = save_png(WPE_BUFFER_SHM(b), wpe_buffer_get_width(b), wpe_buffer_get_height(b)) ? 0 : 3;
    g_main_loop_quit(loop); return G_SOURCE_REMOVE;
}
static void on_title(GObject *o, GParamSpec *p, gpointer d) {
    const char *t = webkit_web_view_get_title(WEBKIT_WEB_VIEW(o)); printf("title: %s\n", t ? t : "(null)"); fflush(stdout);
    if (t && !strcmp(t, "js-ok-42") && !title_ok) { title_ok = TRUE; g_timeout_add(6000, snap, NULL); }
}
static void js_done(GObject *o, GAsyncResult *r, gpointer d) {
    GError *e = NULL; JSCValue *v = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(o), r, &e);
    if (v) { char *t = jsc_value_to_string(v); printf("js says: %s\n", t); g_free(t); } else printf("js error: %s\n", e->message); fflush(stdout);
}
static void on_load(WebKitWebView *v, WebKitLoadEvent e, gpointer d) { printf("load-changed %d title=%s\n", e, webkit_web_view_get_title(v)); fflush(stdout);
    if (e == WEBKIT_LOAD_FINISHED) webkit_web_view_evaluate_javascript(v, "document.title + '|' + document.body.style.background + '|' + (6*7) + '|loop2e7_ms=' + (function(){var t=Date.now(),s=0;for(var i=0;i<2e7;i++)s=(s+i*7)%1000003;return Date.now()-t})()", -1, NULL, NULL, NULL, js_done, NULL); }
static gboolean on_fail(WebKitWebView *v, WebKitLoadEvent e, char *uri, GError *err, gpointer d) { printf("load-failed %s\n", err->message); return FALSE; }
static void on_term(WebKitWebView *v, WebKitWebProcessTerminationReason r, gpointer d) { printf("web-process-terminated %d\n", r); fflush(stdout); }
static gboolean timeout(gpointer d) { puts("timeout"); exit_code = 4; g_main_loop_quit(loop); return G_SOURCE_REMOVE; }

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: p0 URL out.png\n"); return 64; }
    out_png = argv[2]; loop = g_main_loop_new(NULL, FALSE);
    WPEDisplay *disp = g_object_new(thir_display_get_type(), NULL);
    GError *err = NULL; if (!wpe_display_connect(disp, &err)) { fprintf(stderr, "connect: %s\n", err->message); return 1; }
    WebKitNetworkSession *ns = webkit_network_session_new_ephemeral();
    WebKitWebView *wv = g_object_new(WEBKIT_TYPE_WEB_VIEW, "display", disp, "network-session", ns, NULL);
    theview = webkit_web_view_get_wpe_view(wv);
    printf("toplevel=%p\n", (void*)wpe_view_get_toplevel(theview));
    if (!wpe_view_get_toplevel(theview)) { WPEToplevel *tl = wpe_display_create_toplevel(disp, 1); wpe_view_set_toplevel(theview, tl); }
    wpe_toplevel_resize(wpe_view_get_toplevel(theview), W, H); wpe_view_resized(theview, W, H); wpe_view_map(theview); wpe_view_focus_in(theview);
    printf("mapped=%d visible=%d size=%dx%d\n", wpe_view_get_mapped(theview), wpe_view_get_visible(theview), wpe_view_get_width(theview), wpe_view_get_height(theview));
    g_signal_connect(wv, "load-changed", G_CALLBACK(on_load), NULL); g_signal_connect(wv, "load-failed", G_CALLBACK(on_fail), NULL); g_signal_connect(wv, "web-process-terminated", G_CALLBACK(on_term), NULL);
    g_signal_connect(wv, "notify::title", G_CALLBACK(on_title), NULL);
    if (getenv("P0_LOAD_URI")) webkit_web_view_load_uri(wv, argv[1]);
    else { gchar *html = NULL; if (g_file_get_contents(argv[1] + 7, &html, NULL, NULL)) webkit_web_view_load_html(wv, html, argv[1]); else { puts("cannot read page"); return 5; } }
    g_timeout_add_seconds(argc > 3 ? atoi(argv[3]) : 60, timeout, NULL);
    g_main_loop_run(loop);
    printf("shm=%d title_ok=%d exit=%d\n", saw_shm, title_ok, exit_code);
    return exit_code;
}
