// The calls between the Rust host (src/main.rs) and platform.c. Everything runs on the GLib main loop thread.
#pragma once
#include <stdint.h>
typedef struct {
    void *ud;
    // a finished frame of the shown tab: BGRA pixels of the page, `stride` bytes per row, w x h; n damaged rects (x, y, w, h) or n = 0 for "everything". NULL = headless: frames are dropped.
    void (*frame)(const uint8_t *px, uint32_t stride, int w, int h, const int *rects, unsigned n, void *ud);
    // the tab-bound callbacks carry the tab id (thirweb_tab_new); id 0 = not a tab (downloads)
    void (*title)(int id, const char *title, void *ud);
    void (*uri)(int id, const char *uri, void *ud);
    void (*progress)(int id, double p, void *ud);
    void (*load)(int id, int event, void *ud);   // WebKitLoadEvent: 0 started, 1 redirected, 2 committed, 3 finished
    void (*failed)(int id, const char *message, void *ud);   // a load error, a TLS error, a notice: shown in the strip
    void (*download)(void *dl, const char *name, int64_t size, const char *url, int ranges, void *ud);   // size -1 = unknown; ranges: the server said Accept-Ranges: bytes; answer with thirweb_download_answer
    int (*popup_ok)(void *ud);   // a page wants a new tab (target=_blank, window.open): 1 = allowed. Then opened() tells the id; the host shows it.
    void (*opened)(int id, void *ud);
    void (*closed)(int id, void *ud);   // window.close()
    void (*dialog)(int id, int type, const char *message, const char *dflt, void *dlg, void *ud);   // type: 0 alert, 1 confirm, 2 prompt, 3 beforeunload; answer with thirweb_dialog_answer
    void (*chooser)(int id, void *req, void *ud);   // <input type=file>; answer with thirweb_chooser_answer
    void (*found)(int count, void *ud);   // the find: number of matches (0 = none)
    void (*option_menu)(int id, void *menu, int x, int y, int w, int h, void *ud);   // a page's <select>: the popup is the host's; read it with thirweb_menu_*, answer with thirweb_menu_answer. x y w h: the element, in page pixels
    // a download the host knows (by id): kind 1 progress (a = bytes of this piece), 2 finished, 3 failed (a = 1 when the user cancelled; s = message), 4 the answer to a resumed piece (a = status, s = Content-Range or "")
    void (*dl_event)(int kind, unsigned id, int64_t a, const char *s, void *ud);
} ThirCb;
int thirweb_init(int w, int h, double scale, const char *data_dir, const char *cache_dir, const ThirCb *cb);
int thirweb_tab_new(const char *uri_or_null); void thirweb_tab_show(int id); void thirweb_tab_close(int id);
void thirweb_load(const char *uri); void thirweb_reload(void); void thirweb_stop(void); void thirweb_back(void); void thirweb_forward(void);
void thirweb_edit(int cmd);   // 0 copy, 1 cut, 2 paste, 3 select all
void thirweb_find(const char *text, int dir);   // 0 new search, 1 next, 2 previous, 3 close
void thirweb_dialog_answer(void *dlg, int ok, const char *text); void thirweb_chooser_answer(void *req, const char *path);
void thirweb_resize(int w, int h, double scale); void thirweb_focus(int on);
void thirweb_frame_done(void);
void thirweb_download_answer(void *dl, const char *path, unsigned id);   // path NULL = refuse; id: the host's number for it
void thirweb_download_cancel(unsigned id);
void thirweb_download_resume(unsigned id, const char *url, int64_t from, const char *path);   // a new request with Range: bytes=from-, written to path
int thirweb_menu_count(void *menu); void thirweb_menu_item(void *menu, int i, const char **label, int *flags);   // flags: 1 enabled, 2 group heading, 4 selected
void thirweb_menu_answer(void *menu, int index);   // index -1 = closed without a choice
void thirweb_repaint(void);   // paint the page again (after a view covered it)
void thirweb_clip_set(const char *text); char *thirweb_clip_get(void);   // the browser's clipboard (text); free the result with g_free
uint32_t thirweb_keyinfo(uint32_t code, uint32_t value, uint32_t *sym); void thirweb_keysend(uint32_t code, uint32_t value);
void thirweb_pointer(double x, double y); void thirweb_button(uint32_t evdev, int pressed, double x, double y); void thirweb_scroll(double dx, double dy, double x, double y);
void thirweb_watch_fd(int fd, void (*f)(void *), void *d); void thirweb_timeout(unsigned ms, void (*f)(void *), void *d);
void thirweb_run(void); void thirweb_quit(void);
