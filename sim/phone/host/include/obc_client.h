#ifndef OBC_CLIENT_H
#define OBC_CLIENT_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

/* One handle belongs to one Swift actor. Copy returned buffers before an await: they remain
 * valid until obc_client_next or close, not beyond it. No Host/card is required. */
typedef struct ObcClient ObcClient;
typedef struct {
    uint32_t opcode, kind;
    uint64_t object_id, revision, length;
    uint32_t crc, scoped;
    uint8_t store[16], replacement[16];
    const uint8_t *name;
    size_t name_len;
} ObcClientRequest;
typedef struct {
    uint64_t object_id, revision, length;
    uint32_t crc;
    uint16_t kind, flags;
    size_t name_len;
    uint8_t name[48];
} ObcClientEntry;
typedef struct {
    uint32_t opcode, error;
    uint16_t detail, remote_code;
    uint64_t context;
    uint8_t store[16], previous[16];
    uint64_t object_id, revision, length;
    uint32_t crc;
    uint64_t sequence;
    uint32_t timestamp, state, flag;
    const ObcClientEntry *entries;
    size_t entry_count;
} ObcClientResult;
typedef struct {
    uint32_t kind, channel;
    uint64_t token, offset;
    size_t length;
    uint64_t total;
    const uint8_t *bytes;
    ObcClientResult result;
} ObcClientAction;

/* Action kinds: 0 none, 1 Send, 2 ReadSource, 3 WriteSink, 4 ResetSink, 5 Progress,
 * 6 ResetChannels, 7 Restore, 8 primary completion, 9 query completion. Channels: 0 control, 1 stream.
 * Error codes: 0 success, 1 busy, 2 invalid input, 3 protocol, 4 checksum, 5 remote,
 * 6 timeout, 7 cancelled, 8 I/O, 9 link lost, 10 store changed, 11 catalogue changed,
 * 12 not committed, 13 unknown outcome, 14 IDs exhausted. Remote errors carry the wire refusal. */
ObcClient *obc_client_open(size_t control_ceiling, size_t stream_ceiling);
void obc_client_close(ObcClient *client);
ObcClientResult obc_client_start(ObcClient *client, const ObcClientRequest *request,
                               bool query, uint64_t timeout_ms, bool bounded, uint64_t now_ms);
/* Events: 1 Control, 2 Stream, 3 Source, 4 Written, 5 SinkWritten, 6 Tick,
 * 7 Cancel, 8 IoFailed, 9 LinkLost, 10 Restored. Only 1..3 consume bytes.
 * SinkWritten uses offset/length. Restored uses offset=control ceiling, length=stream ceiling. */
ObcClientResult obc_client_event(ObcClient *client, uint32_t kind, uint64_t token, uint64_t offset,
                               const uint8_t *bytes, size_t length, uint64_t now_ms);
bool obc_client_cancel_query(ObcClient *client, uint64_t query_id);
ObcClientAction obc_client_next(ObcClient *client);
/* Input expectations: bit 0 control, bit 1 stream. */
uint32_t obc_client_reads(const ObcClient *client);
bool obc_client_deadline(const ObcClient *client, uint64_t *deadline_ms);
uint32_t obc_client_transfer(const ObcClient *client);
uint32_t obc_client_crc32(const uint8_t *bytes, size_t length);

#ifdef __cplusplus
}
#endif
#endif
