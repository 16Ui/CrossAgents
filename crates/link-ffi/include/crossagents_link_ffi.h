#ifndef CROSSAGENTS_LINK_FFI_H
#define CROSSAGENTS_LINK_FFI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct CaTunnelHandle CaTunnelHandle;
typedef void (*CaEventCallback)(const char *event_json, void *user_data);

/// Decode and preview a `ca1-...` pairing code locally without network access.
/// Returns a heap-allocated UTF-8 JSON string; free with `ca_link_string_free`.
char *ca_link_decode_invite(const char *code);

/// Redeem a one-time `ca1-...` pairing code over iroh.
/// `proxy` may be NULL or empty to use direct/system routing.
/// Returns a heap-allocated UTF-8 JSON string; free with `ca_link_string_free`.
char *ca_link_redeem(
    const char *code,
    const char *device_name,
    const char *os,
    const char *proxy
);

/// Start an in-process local loopback forwarder (`127.0.0.1:0`) tunnelling to the hub over iroh.
/// `config_json` must be JSON:
///   {"endpoint":"...","relay":null,"custom_relay":false,"addrs":[],"token":"cad_...","proxy":null}
/// `callback` receives one JSON event string per event from a background thread.
/// Stop and free with `ca_link_stop`.
CaTunnelHandle *ca_link_start(
    const char *config_json,
    CaEventCallback callback,
    void *user_data
);

/// Stop a running tunnel and free its handle. Safe to call with NULL.
void ca_link_stop(CaTunnelHandle *handle);

/// Free a string returned by `ca_link_decode_invite` or `ca_link_redeem`. Safe to call with NULL.
void ca_link_string_free(char *ptr);

#ifdef __cplusplus
}
#endif

#endif /* CROSSAGENTS_LINK_FFI_H */
