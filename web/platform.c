// thir-web: the WebKit side, written from the public WPEPlatform API (see web/spike/p0.c). A custom WPE display with one toplevel and one view per tab; the visible tab hands
// every finished frame (a WPEBufferSHM) to the host. The calls the Rust host makes: tabs, load, history, find, dialogs, uploads, resize, keys, pointer, downloads. Single threaded, one GLib main loop.
#include "platform.h"
#include <wpe/webkit.h>
#include <wpe/wpe-platform.h>
#include <libsoup/soup.h>
#include <xkbcommon/xkbcommon.h>
#include <glib-unix.h>
#include <string.h>
#include <stdio.h>

typedef struct { int id; WebKitWebView *web; WPEView *view; WPEToplevel *top; WPEBuffer *held; } Tab;   // held: the frame the host is showing; WebKit gets it back (rendered, released) when den says DONE
static ThirCb cb; static GMainLoop *loop; static WPEDisplay *display; static WebKitNetworkSession *nsession;
static GPtrArray *tabs; static Tab *cur; static int next_id = 1, cur_w = 1024, cur_h = 768, cur_focus = 1;
static struct xkb_context *xctx; static struct xkb_keymap *xmap; static struct xkb_state *xstate;
static uint32_t last_sym, last_utf, buttons; static double scale_f = 1.0;

/* ---- toplevel: no window; WebKit asks for sizes, we confirm ---- */
typedef struct { WPEToplevel p; } ThirToplevel; typedef struct { WPEToplevelClass p; } ThirToplevelClass;
G_DEFINE_TYPE(ThirToplevel, thir_toplevel, WPE_TYPE_TOPLEVEL)
static gboolean tl_resize(WPEToplevel *t, int w, int h) { wpe_toplevel_resized(t, w, h); return TRUE; }
static void thir_toplevel_init(ThirToplevel *t) {}
static void thir_toplevel_class_init(ThirToplevelClass *k) { WPE_TOPLEVEL_CLASS(k)->resize = tl_resize; }

/* ---- view: receives the buffers ---- */
typedef struct { WPEView p; } ThirView; typedef struct { WPEViewClass p; } ThirViewClass;
G_DEFINE_TYPE(ThirView, thir_view, WPE_TYPE_VIEW)
static void thir_view_init(ThirView *v) {}
static void give_back(WPEView *v, WPEBuffer *b) { wpe_view_buffer_rendered(v, b); wpe_view_buffer_released(v, b); g_object_unref(b); }
typedef struct { WPEView *v; WPEBuffer *b; } Back;
static gboolean idle_back(gpointer p) { Back *k = p; give_back(k->v, k->b); g_object_unref(k->v); g_free(k); return G_SOURCE_REMOVE; }
static gboolean view_render(WPEView *v, WPEBuffer *b, const WPERectangle *d, guint n, GError **err) {
    if (!WPE_IS_BUFFER_SHM(b)) { g_set_error_literal(err, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED, "only shared-memory frames"); return FALSE; }
    Tab *t = g_object_get_data(G_OBJECT(v), "tab");
    if (!cb.frame || !t || t != cur) { Back *k = g_new(Back, 1); k->v = g_object_ref(v); k->b = g_object_ref(b); g_idle_add(idle_back, k); return TRUE; }   // headless or a tab nobody looks at: drop the frame
    if (t->held) { give_back(v, t->held); t->held = NULL; }
    int rects[4 * 32]; guint k = n > 32 ? 0 : n;   // more than 32 rects (or none): the host copies the whole frame
    for (guint i = 0; i < k; i++) { rects[4*i] = d[i].x; rects[4*i+1] = d[i].y; rects[4*i+2] = d[i].width; rects[4*i+3] = d[i].height; }
    t->held = g_object_ref(b);
    gsize len; const guchar *px = g_bytes_get_data(wpe_buffer_shm_get_data(WPE_BUFFER_SHM(b)), &len);
    cb.frame(px, wpe_buffer_shm_get_stride(WPE_BUFFER_SHM(b)), wpe_buffer_get_width(b), wpe_buffer_get_height(b), rects, k, cb.ud);
    return TRUE;
}
static void thir_view_class_init(ThirViewClass *k) { WPE_VIEW_CLASS(k)->render_buffer = view_render; }
void thirweb_frame_done(void) { if (cur && cur->held) { WPEBuffer *b = cur->held; cur->held = NULL; give_back(cur->view, b); } }

/* ---- display: no EGL, no DRM, no input devices ---- */
typedef struct { WPEDisplay p; } ThirDisplay; typedef struct { WPEDisplayClass p; } ThirDisplayClass;
G_DEFINE_TYPE(ThirDisplay, thir_display, WPE_TYPE_DISPLAY)
static void thir_display_init(ThirDisplay *d) {}
static gboolean d_connect(WPEDisplay *d, GError **e) { return TRUE; }
static WPEView *d_create_view(WPEDisplay *d) { return g_object_new(thir_view_get_type(), "display", d, NULL); }
static WPEToplevel *d_create_toplevel(WPEDisplay *d, guint m) { return g_object_new(thir_toplevel_get_type(), "display", d, NULL); }
static WPEClipboard *d_get_clipboard(WPEDisplay *d) { static WPEClipboard *c; if (!c) c = wpe_clipboard_new(d); return c; }   // one clipboard for the strip and every page
static void thir_display_class_init(ThirDisplayClass *k) { WPEDisplayClass *c = WPE_DISPLAY_CLASS(k); c->get_clipboard = d_get_clipboard; c->connect = d_connect; c->create_view = d_create_view; c->create_toplevel = d_create_toplevel; }

/* ---- web view signals (one set per tab; the tab is the user data) ---- */
static Tab *tab_by_id(int id) { for (guint i = 0; i < tabs->len; i++) { Tab *t = g_ptr_array_index(tabs, i); if (t->id == id) return t; } return NULL; }
static void on_title(GObject *o, GParamSpec *p, gpointer d) { Tab *t = d; const char *s = webkit_web_view_get_title(t->web); if (cb.title) cb.title(t->id, s ? s : "", cb.ud); }
static void on_uri(GObject *o, GParamSpec *p, gpointer d) { Tab *t = d; const char *u = webkit_web_view_get_uri(t->web); if (cb.uri) cb.uri(t->id, u ? u : "", cb.ud); }
static void on_progress(GObject *o, GParamSpec *p, gpointer d) { Tab *t = d; if (cb.progress) cb.progress(t->id, webkit_web_view_get_estimated_load_progress(t->web), cb.ud); }
static void on_load(WebKitWebView *v, WebKitLoadEvent e, gpointer d) { Tab *t = d; if (cb.load) cb.load(t->id, (int)e, cb.ud); }
static void error_page(Tab *t, const char *uri, const char *msg) {   // the message in the page as well as in the strip; the address stays the failing one
    char *m = g_markup_escape_text(msg, -1);
    char *h = g_strdup_printf("<!doctype html><meta charset=utf-8><title>Cannot open this page</title><body style=\"font:20px sans-serif;margin:3em;color:#222\"><h1>Cannot open this page</h1><p>%s</p><p style=\"color:#666\">Nothing was loaded and nothing was sent to the site.</p>", m);
    webkit_web_view_load_alternate_html(t->web, h, uri, NULL); g_free(h); g_free(m);
}
static gboolean on_tls(WebKitWebView *v, const char *uri, GTlsCertificate *c, GTlsCertificateFlags f, gpointer d) {   // never "proceed anyway"
    Tab *t = d;
    const char *why = f & G_TLS_CERTIFICATE_UNKNOWN_CA ? "the certificate was signed by an unknown authority" : f & G_TLS_CERTIFICATE_BAD_IDENTITY ? "the certificate is for another host name"
        : f & G_TLS_CERTIFICATE_EXPIRED ? "the certificate has expired" : f & G_TLS_CERTIFICATE_NOT_ACTIVATED ? "the certificate is not valid yet" : f & G_TLS_CERTIFICATE_REVOKED ? "the certificate was revoked" : "the certificate is not trustworthy";
    char *m = g_strdup_printf("TLS error: %s (%s)", why, uri); if (cb.failed) cb.failed(t->id, m, cb.ud); error_page(t, uri, m); g_free(m); return TRUE;
}
static gboolean on_failed(WebKitWebView *v, WebKitLoadEvent e, const char *uri, GError *err, gpointer d) {
    Tab *t = d;
    if (g_error_matches(err, WEBKIT_NETWORK_ERROR, WEBKIT_NETWORK_ERROR_CANCELLED) || g_error_matches(err, WEBKIT_POLICY_ERROR, WEBKIT_POLICY_ERROR_FRAME_LOAD_INTERRUPTED_BY_POLICY_CHANGE)) return TRUE;   // a stop, or a download taking over
    char *m = g_strdup_printf("%s (%s)", err->message, uri); if (cb.failed) cb.failed(t->id, m, cb.ud); error_page(t, uri, m); g_free(m); return TRUE;
}
static gboolean on_permission(WebKitWebView *v, WebKitPermissionRequest *r, gpointer d) { webkit_permission_request_deny(r); return TRUE; }   // camera, microphone, location, notifications: never
static gboolean on_notify(WebKitWebView *v, WebKitNotification *n, gpointer d) { return TRUE; }
static void on_term(WebKitWebView *v, WebKitWebProcessTerminationReason r, gpointer d) { Tab *t = d; if (cb.failed) cb.failed(t->id, "the page process stopped", cb.ud); }
static gboolean on_dialog(WebKitWebView *v, WebKitScriptDialog *dl, gpointer d) {   // alert, confirm, prompt, beforeunload: the host asks in the strip; the page waits, nothing else does
    Tab *t = d; if (!cb.dialog) return FALSE;
    int type = webkit_script_dialog_get_dialog_type(dl);
    cb.dialog(t->id, type, webkit_script_dialog_get_message(dl), type == WEBKIT_SCRIPT_DIALOG_PROMPT ? webkit_script_dialog_prompt_get_default_text(dl) : "", webkit_script_dialog_ref(dl), cb.ud); return TRUE;
}
void thirweb_dialog_answer(void *dl, int ok, const char *text) {   // ok: OK / yes; text: the prompt's answer (NULL = cancelled)
    WebKitScriptDialog *d = dl; int type = webkit_script_dialog_get_dialog_type(d);
    if (type == WEBKIT_SCRIPT_DIALOG_CONFIRM || type == WEBKIT_SCRIPT_DIALOG_BEFORE_UNLOAD_CONFIRM) webkit_script_dialog_confirm_set_confirmed(d, ok);
    if (type == WEBKIT_SCRIPT_DIALOG_PROMPT) webkit_script_dialog_prompt_set_text(d, ok ? text : NULL);
    webkit_script_dialog_close(d); webkit_script_dialog_unref(d);
}
static gboolean on_chooser(WebKitWebView *v, WebKitFileChooserRequest *r, gpointer d) {
    Tab *t = d; if (!cb.chooser) return FALSE;
    cb.chooser(t->id, g_object_ref(r), cb.ud); return TRUE;
}
void thirweb_chooser_answer(void *req, const char *path) {   // a path inside ~/Downloads, or NULL = cancel
    WebKitFileChooserRequest *r = req;
    if (path) { const char *files[2] = { path, NULL }; webkit_file_chooser_request_select_files(r, files); } else webkit_file_chooser_request_cancel(r);
    g_object_unref(r);
}
static void on_found(WebKitFindController *fc, guint n, gpointer d) { Tab *t = d; if (cb.found && t == cur) cb.found((int)n, cb.ud); }
static void on_nofind(WebKitFindController *fc, gpointer d) { Tab *t = d; if (cb.found && t == cur) cb.found(0, cb.ud); }
static void on_close(WebKitWebView *v, gpointer d) { Tab *t = d; if (cb.closed) cb.closed(t->id, cb.ud); }   // window.close()
static WebKitWebView *on_create(WebKitWebView *v, WebKitNavigationAction *a, gpointer d);

/* ---- downloads: the host asks the user; nothing is written before the answer. A paused download is cancelled; a resume is a Range request loaded in a hidden view (WebKit's download API has no headers) ---- */
static GHashTable *dls;   // host id -> WebKitDownload
static unsigned resume_id; static char *resume_path; static WebKitWebView *resume_view;
static gboolean dl_destination(WebKitDownload *dl, const char *name, gpointer d) {
    WebKitURIResponse *r = webkit_download_get_response(dl); guint64 len = webkit_uri_response_get_content_length(r);
    if (resume_id && resume_path) {   // the answer to a Range request: tell the host, then it goes to its file
        SoupMessageHeaders *h = webkit_uri_response_get_http_headers(r); const char *cr = h ? soup_message_headers_get_one(h, "Content-Range") : NULL;
        unsigned id = resume_id; char *path = resume_path; resume_id = 0; resume_path = NULL;
        g_object_set_data(G_OBJECT(dl), "id", GUINT_TO_POINTER(id)); g_hash_table_insert(dls, GUINT_TO_POINTER(id), g_object_ref(dl));
        if (cb.dl_event) cb.dl_event(4, id, webkit_uri_response_get_status_code(r), cr ? cr : "", cb.ud);
        if (webkit_uri_response_get_status_code(r) == 206) { webkit_download_set_allow_overwrite(dl, TRUE); webkit_download_set_destination(dl, path); } else webkit_download_cancel(dl);
        g_free(path); return TRUE;
    }
    if (!cb.download) { webkit_download_cancel(dl); return TRUE; }
    SoupMessageHeaders *h = webkit_uri_response_get_http_headers(r); const char *ar = h ? soup_message_headers_get_one(h, "Accept-Ranges") : NULL;
    cb.download(g_object_ref(dl), name, len ? (int64_t)len : -1, webkit_uri_request_get_uri(webkit_download_get_request(dl)), ar && strstr(ar, "bytes") ? 1 : 0, cb.ud); return TRUE;
}
static gboolean unref_idle(gpointer o) { g_object_unref(o); return G_SOURCE_REMOVE; }
static unsigned dl_id(WebKitDownload *dl) { return GPOINTER_TO_UINT(g_object_get_data(G_OBJECT(dl), "id")); }
static void dl_forget(WebKitDownload *dl) { unsigned id = dl_id(dl); if (id) g_hash_table_remove(dls, GUINT_TO_POINTER(id)); }
static void dl_received(WebKitDownload *dl, guint64 n, gpointer d) {
    static gint64 last; gint64 t = g_get_monotonic_time(); unsigned id = dl_id(dl); if (!id || t - last < 250000) return; last = t;
    if (cb.dl_event) cb.dl_event(1, id, (int64_t)webkit_download_get_received_data_length(dl), "", cb.ud);
}
static void dl_done(WebKitDownload *dl, gpointer d) { unsigned id = dl_id(dl); if (id && cb.dl_event) cb.dl_event(2, id, (int64_t)webkit_download_get_received_data_length(dl), "", cb.ud); dl_forget(dl); }
static void dl_failed(WebKitDownload *dl, GError *e, gpointer d) {
    unsigned id = dl_id(dl); int user = g_error_matches(e, WEBKIT_DOWNLOAD_ERROR, WEBKIT_DOWNLOAD_ERROR_CANCELLED_BY_USER);
    if (id && cb.dl_event) cb.dl_event(3, id, user, e->message, cb.ud);
    dl_forget(dl);
}
static void on_download(WebKitNetworkSession *s, WebKitDownload *dl, gpointer d) {
    g_signal_connect(dl, "decide-destination", G_CALLBACK(dl_destination), NULL); g_signal_connect(dl, "received-data", G_CALLBACK(dl_received), NULL);
    g_signal_connect(dl, "finished", G_CALLBACK(dl_done), NULL); g_signal_connect(dl, "failed", G_CALLBACK(dl_failed), NULL);
    if (resume_view) { WebKitWebView *v = resume_view; resume_view = NULL; g_idle_add(unref_idle, v); }   // the hidden view has done its work
}
void thirweb_download_answer(void *dl, const char *path, unsigned id) {
    if (path) { g_object_set_data(G_OBJECT(dl), "id", GUINT_TO_POINTER(id)); g_hash_table_insert(dls, GUINT_TO_POINTER(id), g_object_ref(dl)); webkit_download_set_allow_overwrite(dl, TRUE); webkit_download_set_destination(dl, path); }
    else webkit_download_cancel(dl);
    g_object_unref(dl);
}
void thirweb_download_cancel(unsigned id) { WebKitDownload *dl = g_hash_table_lookup(dls, GUINT_TO_POINTER(id)); if (dl) webkit_download_cancel(dl); }
static gboolean resume_policy(WebKitWebView *v, WebKitPolicyDecision *dec, WebKitPolicyDecisionType t, gpointer d) { if (t != WEBKIT_POLICY_DECISION_TYPE_RESPONSE) return FALSE; webkit_policy_decision_download(dec); return TRUE; }
void thirweb_download_resume(unsigned id, const char *url, int64_t from, const char *path) {
    if (resume_view) g_object_unref(resume_view);
    resume_view = g_object_new(WEBKIT_TYPE_WEB_VIEW, "display", display, "network-session", nsession, NULL);
    { WPEView *rv = webkit_web_view_get_wpe_view(resume_view); if (!wpe_view_get_toplevel(rv)) wpe_view_set_toplevel(rv, wpe_display_create_toplevel(display, 1)); wpe_view_resized(rv, 100, 100); }
    g_signal_connect(resume_view, "decide-policy", G_CALLBACK(resume_policy), NULL);
    g_free(resume_path); resume_id = id; resume_path = g_strdup(path);
    WebKitURIRequest *rq = webkit_uri_request_new(url); char *r = g_strdup_printf("bytes=%" G_GINT64_FORMAT "-", from);
    soup_message_headers_append(webkit_uri_request_get_http_headers(rq), "Range", r); g_free(r);
    webkit_web_view_load_request(resume_view, rq); g_object_unref(rq);
}

/* ---- <select>: the page's option menu is the host's to show ---- */
static gboolean on_option_menu(WebKitWebView *v, WebKitOptionMenu *m, WebKitRectangle *r, gpointer d) {
    Tab *t = d; if (!cb.option_menu || t != cur) return FALSE;
    cb.option_menu(t->id, g_object_ref(m), r->x, r->y, r->width, r->height, cb.ud); return TRUE;
}
int thirweb_menu_count(void *m) { return (int)webkit_option_menu_get_n_items(m); }
void thirweb_menu_item(void *m, int i, const char **label, int *flags) {
    WebKitOptionMenuItem *it = webkit_option_menu_get_item(m, i);
    *label = webkit_option_menu_item_get_label(it);
    *flags = (webkit_option_menu_item_is_enabled(it) ? 1 : 0) | (webkit_option_menu_item_is_group_label(it) ? 2 : 0) | (webkit_option_menu_item_is_selected(it) ? 4 : 0);
}
void thirweb_menu_answer(void *m, int index) { if (index >= 0) webkit_option_menu_activate_item(m, index); webkit_option_menu_close(m); g_object_unref(m); }

/* ---- clipboard: WPE's own, in this process; text only ---- */
void thirweb_clip_set(const char *text) {
    WPEClipboard *c = wpe_display_get_clipboard(display); if (!c) return;
    WPEClipboardContent *k = wpe_clipboard_content_new(); wpe_clipboard_content_set_text(k, text); wpe_clipboard_set_content(c, k); wpe_clipboard_content_unref(k);
}
char *thirweb_clip_get(void) {   // whatever text format is there (a page copies as text/plain;charset=utf-8, the strip as text/plain)
    WPEClipboard *c = wpe_display_get_clipboard(display); if (!c) return NULL;
    const char *const *f = wpe_clipboard_get_formats(c); gsize n = 0;
    for (; f && *f; f++) if (g_str_has_prefix(*f, "text/plain")) return wpe_clipboard_read_text(c, *f, &n);
    return NULL;
}
void thirweb_repaint(void) {   // WebKit paints nothing new for a view that is unchanged: a size change of one row forces a full frame
    if (!cur) return;
    wpe_toplevel_resize(cur->top, cur_w, cur_h > 1 ? cur_h - 1 : cur_h); wpe_view_resized(cur->view, cur_w, cur_h > 1 ? cur_h - 1 : cur_h);
    wpe_toplevel_resize(cur->top, cur_w, cur_h); wpe_view_resized(cur->view, cur_w, cur_h);
}

static void focus_in(Tab *t) { wpe_toplevel_state_changed(t->top, WPE_TOPLEVEL_STATE_ACTIVE); wpe_view_focus_in(t->view); }   // an inactive window draws no text caret
static void focus_out(Tab *t) { wpe_view_focus_out(t->view); wpe_toplevel_state_changed(t->top, WPE_TOPLEVEL_STATE_NONE); }
/* ---- tabs: one WebKitWebView and one WPEView each; only the shown tab is mapped and its frames reach the host ---- */
static Tab *tab_make(WebKitWebView *related) {
    Tab *t = g_new0(Tab, 1); t->id = next_id++;
    // a video or a sound plays when the page says autoplay (YouTube, a <video autoplay>): this is a desktop with its own volume key, not a phone
    static WebKitWebsitePolicies *pol; if (!pol) pol = webkit_website_policies_new_with_policies("autoplay", WEBKIT_AUTOPLAY_ALLOW, NULL);
    t->web = related ? g_object_new(WEBKIT_TYPE_WEB_VIEW, "related-view", related, "website-policies", pol, NULL) : g_object_new(WEBKIT_TYPE_WEB_VIEW, "display", display, "network-session", nsession, "website-policies", pol, NULL);
    WebKitSettings *st = webkit_web_view_get_settings(t->web);
    webkit_settings_set_javascript_can_open_windows_automatically(st, FALSE); webkit_settings_set_enable_developer_extras(st, FALSE);
    t->view = webkit_web_view_get_wpe_view(t->web); g_object_set_data(G_OBJECT(t->view), "tab", t);
    t->top = wpe_view_get_toplevel(t->view);
    if (!t->top) { t->top = wpe_display_create_toplevel(display, 1); wpe_view_set_toplevel(t->view, t->top); }
    wpe_toplevel_scale_changed(t->top, scale_f); wpe_toplevel_resize(t->top, cur_w, cur_h); wpe_view_resized(t->view, cur_w, cur_h);
    WebKitWebView *w = t->web;
    g_signal_connect(w, "notify::title", G_CALLBACK(on_title), t); g_signal_connect(w, "notify::uri", G_CALLBACK(on_uri), t);
    g_signal_connect(w, "notify::estimated-load-progress", G_CALLBACK(on_progress), t); g_signal_connect(w, "load-changed", G_CALLBACK(on_load), t);
    g_signal_connect(w, "load-failed", G_CALLBACK(on_failed), t); g_signal_connect(w, "load-failed-with-tls-errors", G_CALLBACK(on_tls), t);
    g_signal_connect(w, "permission-request", G_CALLBACK(on_permission), t); g_signal_connect(w, "show-notification", G_CALLBACK(on_notify), t);
    g_signal_connect(w, "web-process-terminated", G_CALLBACK(on_term), t); g_signal_connect(w, "script-dialog", G_CALLBACK(on_dialog), t);
    g_signal_connect(w, "run-file-chooser", G_CALLBACK(on_chooser), t); g_signal_connect(w, "close", G_CALLBACK(on_close), t); g_signal_connect(w, "show-option-menu", G_CALLBACK(on_option_menu), t); g_signal_connect(w, "create", G_CALLBACK(on_create), t);
    WebKitFindController *fc = webkit_web_view_get_find_controller(w);
    g_signal_connect(fc, "found-text", G_CALLBACK(on_found), t); g_signal_connect(fc, "counted-matches", G_CALLBACK(on_found), t); g_signal_connect(fc, "failed-to-find-text", G_CALLBACK(on_nofind), t);
    g_ptr_array_add(tabs, t); return t;
}
static WebKitWebView *on_create(WebKitWebView *v, WebKitNavigationAction *a, gpointer d) {   // target=_blank, window.open: a new tab, unless the host says no (too many)
    if (!cb.popup_ok || !cb.popup_ok(cb.ud)) return NULL;
    Tab *t = tab_make(v); if (cb.opened) cb.opened(t->id, cb.ud); return t->web;
}
int thirweb_tab_new(const char *uri) { Tab *t = tab_make(NULL); if (uri) webkit_web_view_load_uri(t->web, uri); return t->id; }
void thirweb_tab_show(int id) {
    Tab *t = tab_by_id(id); if (!t || t == cur) return;
    if (cur) { thirweb_frame_done(); focus_out(cur); wpe_view_unmap(cur->view); }
    cur = t; wpe_toplevel_scale_changed(t->top, scale_f);
    // WebKit paints nothing new for a view that is only mapped again, and the pane still shows the tab before: a size change of one row forces a full frame
    wpe_toplevel_resize(t->top, cur_w, cur_h > 1 ? cur_h - 1 : cur_h); wpe_view_resized(t->view, cur_w, cur_h > 1 ? cur_h - 1 : cur_h);
    wpe_toplevel_resize(t->top, cur_w, cur_h); wpe_view_resized(t->view, cur_w, cur_h);
    wpe_view_map(t->view); if (cur_focus) focus_in(t);
}
void thirweb_tab_close(int id) {
    Tab *t = tab_by_id(id); if (!t) return;
    if (t == cur) { thirweb_frame_done(); cur = NULL; }
    if (t->held) { give_back(t->view, t->held); t->held = NULL; }
    wpe_view_unmap(t->view); g_ptr_array_remove(tabs, t); g_signal_handlers_disconnect_matched(t->web, G_SIGNAL_MATCH_DATA, 0, 0, NULL, NULL, t);
    g_object_unref(t->web); g_free(t);
}

/* ---- setup ---- */
int thirweb_init(int w, int h, double scale, const char *data_dir, const char *cache_dir, const ThirCb *c) {
    cb = *c; scale_f = scale; cur_w = w; cur_h = h; loop = g_main_loop_new(NULL, FALSE); tabs = g_ptr_array_new();
    xctx = xkb_context_new(XKB_CONTEXT_NO_FLAGS); struct xkb_rule_names rn = { "evdev", "pc105", "us", NULL, NULL };
    xmap = xkb_keymap_new_from_names(xctx, &rn, XKB_KEYMAP_COMPILE_NO_FLAGS); if (!xmap) { fprintf(stderr, "web: no keyboard map (xkb)\n"); return -1; }
    xstate = xkb_state_new(xmap);
    display = g_object_new(thir_display_get_type(), NULL); GError *err = NULL;
    if (!wpe_display_connect(display, &err)) { fprintf(stderr, "web: %s\n", err->message); return -1; }
    nsession = data_dir ? webkit_network_session_new(data_dir, cache_dir) : webkit_network_session_new_ephemeral();
    webkit_network_session_set_tls_errors_policy(nsession, WEBKIT_TLS_ERRORS_POLICY_FAIL);
    if (data_dir) { char *f = g_build_filename(data_dir, "cookies.sqlite", NULL); webkit_cookie_manager_set_persistent_storage(webkit_network_session_get_cookie_manager(nsession), f, WEBKIT_COOKIE_PERSISTENT_STORAGE_SQLITE); g_free(f); }
    dls = g_hash_table_new_full(g_direct_hash, g_direct_equal, NULL, g_object_unref);
    g_signal_connect(nsession, "download-started", G_CALLBACK(on_download), NULL);
    return 0;
}
void thirweb_load(const char *uri) { if (cur) webkit_web_view_load_uri(cur->web, uri); }
void thirweb_reload(void) { if (cur) webkit_web_view_reload(cur->web); }
void thirweb_stop(void) { if (cur) webkit_web_view_stop_loading(cur->web); }
void thirweb_back(void) { if (cur && webkit_web_view_can_go_back(cur->web)) webkit_web_view_go_back(cur->web); }
void thirweb_forward(void) { if (cur && webkit_web_view_can_go_forward(cur->web)) webkit_web_view_go_forward(cur->web); }
void thirweb_edit(int cmd) { if (cur) webkit_web_view_execute_editing_command(cur->web, cmd == 0 ? WEBKIT_EDITING_COMMAND_COPY : cmd == 1 ? WEBKIT_EDITING_COMMAND_CUT : cmd == 2 ? WEBKIT_EDITING_COMMAND_PASTE : WEBKIT_EDITING_COMMAND_SELECT_ALL); }
void thirweb_find(const char *text, int dir) {   // 0 = a new search (from the top), 1 = next, 2 = previous, 3 = close the find
    if (!cur) return;
    WebKitFindController *fc = webkit_web_view_get_find_controller(cur->web);
    if (dir == 3 || !text || !*text) { webkit_find_controller_search_finish(fc); return; }
    if (dir == 1) webkit_find_controller_search_next(fc); else if (dir == 2) webkit_find_controller_search_previous(fc);
    else webkit_find_controller_search(fc, text, WEBKIT_FIND_OPTIONS_CASE_INSENSITIVE | WEBKIT_FIND_OPTIONS_WRAP_AROUND, 1000);
}
void thirweb_resize(int w, int h, double scale) {
    cur_w = w; cur_h = h; scale_f = scale;
    for (guint i = 0; i < tabs->len; i++) {   // every tab: a background tab must have the right size when it is shown
        Tab *t = g_ptr_array_index(tabs, i);
        if (wpe_toplevel_get_scale(t->top) != scale) wpe_toplevel_scale_changed(t->top, scale);
        wpe_toplevel_resize(t->top, w, h); wpe_view_resized(t->view, w, h);
    }
}
void thirweb_focus(int on) { cur_focus = on; if (cur) { if (on) focus_in(cur); else focus_out(cur); } }

/* ---- input: den's evdev events -> WPE events ---- */
static WPEModifiers mods(void) {
    WPEModifiers m = 0;
    if (xkb_state_mod_name_is_active(xstate, XKB_MOD_NAME_CTRL, XKB_STATE_MODS_EFFECTIVE) > 0) m |= WPE_MODIFIER_KEYBOARD_CONTROL;
    if (xkb_state_mod_name_is_active(xstate, XKB_MOD_NAME_SHIFT, XKB_STATE_MODS_EFFECTIVE) > 0) m |= WPE_MODIFIER_KEYBOARD_SHIFT;
    if (xkb_state_mod_name_is_active(xstate, XKB_MOD_NAME_ALT, XKB_STATE_MODS_EFFECTIVE) > 0) m |= WPE_MODIFIER_KEYBOARD_ALT;
    if (xkb_state_mod_name_is_active(xstate, XKB_MOD_NAME_CAPS, XKB_STATE_MODS_EFFECTIVE) > 0) m |= WPE_MODIFIER_KEYBOARD_CAPS_LOCK;
    if (buttons & 1) m |= WPE_MODIFIER_POINTER_BUTTON1;
    if (buttons & 2) m |= WPE_MODIFIER_POINTER_BUTTON2;
    if (buttons & 4) m |= WPE_MODIFIER_POINTER_BUTTON3;
    return m;
}
static guint32 now_ms(void) { return (guint32)(g_get_monotonic_time() / 1000); }
// Updates the keyboard state with an evdev key (value 0 release, 1 press, 2 repeat) and returns the character it types (0 = none); *sym = the keysym.
uint32_t thirweb_keyinfo(uint32_t code, uint32_t value, uint32_t *sym) {
    xkb_keycode_t k = code + 8;
    last_sym = xkb_state_key_get_one_sym(xstate, k); last_utf = value ? xkb_state_key_get_utf32(xstate, k) : 0;
    if (value != 2) xkb_state_update_key(xstate, k, value ? XKB_KEY_DOWN : XKB_KEY_UP);
    if (sym) *sym = last_sym;
    return last_utf;
}
// Sends the key last given to thirweb_keyinfo to the page.
void thirweb_keysend(uint32_t code, uint32_t value) {
    if (!cur) return;
    WPEEvent *e = wpe_event_keyboard_new(value ? WPE_EVENT_KEYBOARD_KEY_DOWN : WPE_EVENT_KEYBOARD_KEY_UP, cur->view, WPE_INPUT_SOURCE_KEYBOARD, now_ms(), mods(), code + 8, last_sym);
    wpe_view_event(cur->view, e); wpe_event_unref(e);
}
void thirweb_pointer(double x, double y) {
    if (!cur) return;
    WPEEvent *e = wpe_event_pointer_move_new(WPE_EVENT_POINTER_MOVE, cur->view, WPE_INPUT_SOURCE_MOUSE, now_ms(), mods(), x, y, 0, 0); wpe_view_event(cur->view, e); wpe_event_unref(e);
}
void thirweb_button(uint32_t evdev, int pressed, double x, double y) {   // BTN_LEFT 0x110, RIGHT 0x111, MIDDLE 0x112; WPE numbers them 1, 3, 2
    if (!cur) return;
    guint b = evdev == 0x110 ? 1 : evdev == 0x111 ? 3 : evdev == 0x112 ? 2 : 0; if (!b) return;
    uint32_t bit = b == 1 ? 1 : b == 3 ? 4 : 2; if (pressed) buttons |= bit;
    static guint32 last; static int count; guint32 t = now_ms(); if (pressed) { count = (t - last < 400) ? count + 1 : 1; last = t; }
    WPEEvent *e = wpe_event_pointer_button_new(pressed ? WPE_EVENT_POINTER_DOWN : WPE_EVENT_POINTER_UP, cur->view, WPE_INPUT_SOURCE_MOUSE, t, mods(), b, x, y, pressed ? count : 0);
    wpe_view_event(cur->view, e); wpe_event_unref(e); if (!pressed) buttons &= ~bit;
}
void thirweb_scroll(double dx, double dy, double x, double y) {   if (!cur) return;   // wheel notches; den: dy > 0 = up, dx > 0 = right; WebKit wheel ticks: dy > 0 = up, dx > 0 = left
    WPEEvent *e = wpe_event_scroll_new(cur->view, WPE_INPUT_SOURCE_MOUSE, now_ms(), mods(), -dx, dy, FALSE, FALSE, x, y); wpe_view_event(cur->view, e); wpe_event_unref(e);
}

/* ---- loop ---- */
typedef struct { void (*f)(void *); void *d; } Watch;
static gboolean fd_ready(gint fd, GIOCondition c, gpointer p) { Watch *w = p; w->f(w->d); return G_SOURCE_CONTINUE; }
void thirweb_watch_fd(int fd, void (*f)(void *), void *d) { Watch *w = g_new(Watch, 1); w->f = f; w->d = d; g_unix_fd_add(fd, G_IO_IN | G_IO_ERR | G_IO_HUP, fd_ready, w); }
static gboolean timeout_cb(gpointer p) { Watch *w = p; w->f(w->d); return G_SOURCE_REMOVE; }
void thirweb_timeout(unsigned ms, void (*f)(void *), void *d) { Watch *w = g_new(Watch, 1); w->f = f; w->d = d; g_timeout_add(ms, timeout_cb, w); }
void thirweb_run(void) { g_main_loop_run(loop); }
void thirweb_quit(void) { g_main_loop_quit(loop); }
