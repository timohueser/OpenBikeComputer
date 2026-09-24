/* The iPhone host's C surface: the real firmware over one persistent card, behind an opaque
 * device. `apps/obc-ios-host/src/ffi.rs` implements exactly these declarations.
 *
 * Every call on a host is main thread only: the display link, the touch areas and the
 * CoreLocation and CoreMotion delegates all run there, and the host does not synchronise. A NULL
 * host is a no-op, false, NULL, or a failure code. A NULL or non-UTF-8 path fails through
 * obc_ios_last_error(), which is thread-local: read it on the thread that made the failing call.
 * A panic prints to stderr and aborts the process. */
#ifndef OBC_IOS_HOST_H
#define OBC_IOS_HOST_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ObcHost ObcHost;
typedef enum { OBC_BUTTON_UP = 0, OBC_BUTTON_DOWN = 1, OBC_BUTTON_SELECT = 2, OBC_BUTTON_BACK = 3 } ObcButton;

/* What is at the card path: 0 no card file, 1 a card with no map, 2 a card with a map, -1 error.
   The shell asks before obc_ios_open() to decide whether to show the import screen. */
int32_t obc_ios_card_state(const char *card_path);

/* Import or replace the card map. Creates the card when the file is absent.
   Call with no host open. Returns 0 on success; read obc_ios_last_error() on failure. */
int32_t obc_ios_import_map(const char *card_path, const char *obcm_path);

/* Those two take no host and may run on any thread while no host is open, so a large map copy does
   not freeze the UI. Everything below is main thread only. */

ObcHost *obc_ios_open(const char *card_path, const char *settings_path, const char *exports_dir); /* NULL on failure */
void obc_ios_close(ObcHost *host); /* exactly once per handle; the handle is dead afterwards */

bool obc_ios_tick(ObcHost *host, double now_ms);                /* true when the frame changed */
const uint8_t *obc_ios_frame(const ObcHost *host);             /* width*height*4 RGBA, opaque alpha; valid until the next call on this host */
/* The newest cue once, as *len mono float samples at sample_rate; NULL and *len 0 until a pass raises
   the next cue. Valid until the next call on this host. */
const float *obc_ios_take_sound(ObcHost *host, uint32_t sample_rate, uint32_t *len);
uint32_t obc_ios_frame_width(void);
uint32_t obc_ios_frame_height(void);

/* NaN marks an unknown course or speed; 0 marks no UTC stamp. */
void obc_ios_push_fix(ObcHost *host, int32_t lat_udeg, int32_t lon_udeg, float course_deg, float speed_mps, uint32_t unix_secs);
void obc_ios_push_heading(ObcHost *host, float degrees);
void obc_ios_push_altitude(ObcHost *host, float metres);
void obc_ios_push_battery(ObcHost *host, uint8_t percent);
void obc_ios_button(ObcHost *host, ObcButton button, bool down);

int32_t obc_ios_import_route(ObcHost *host, const char *path);  /* .obcr or .gpx; 0 on success */
const char *obc_ios_screen(const ObcHost *host);               /* static, NUL-terminated Screen::name() */
bool obc_ios_recording(const ObcHost *host);
const char *obc_ios_last_error(void);                          /* thread-local, NUL-terminated, empty until a call fails; valid until the next failing call */

#ifdef __cplusplus
}
#endif

#endif /* OBC_IOS_HOST_H */
