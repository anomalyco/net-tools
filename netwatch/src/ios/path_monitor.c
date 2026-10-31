// Public Network.framework API. Compile against the SDK so availability and
// the block ABI are checked by Clang, including when targeting older iOS.
#include <Network/Network.h>
#include <netinet/in.h>
#include <string.h>

typedef void (*update_fn)(void *, uint32_t, const struct sockaddr *);
typedef void (*cancel_fn)(void *);

static void report(nw_path_t path, void *context, update_fn update) API_AVAILABLE(ios(12.0)) {
    __block uint32_t index = 0;
    __block struct sockaddr_storage gateway = {0};
    if (nw_path_get_status(path) == nw_path_status_satisfied) {
        // Available interfaces are in preference order, not all in use.
        nw_path_enumerate_interfaces(path, ^bool(nw_interface_t iface) {
            if (nw_path_uses_interface_type(path, nw_interface_get_type(iface))) {
                index = nw_interface_get_index(iface);
                return false;
            }
            return true;
        });
        if (__builtin_available(iOS 13.0, *)) {
            nw_path_enumerate_gateways(path, ^bool(nw_endpoint_t endpoint) {
                const struct sockaddr *addr = nw_endpoint_get_address(endpoint);
                if (!addr) return true;
                // Prefer IPv4 for port mapping. Do not keep an endpoint's
                // borrowed address past this enumeration callback.
                if (addr->sa_family == AF_INET && addr->sa_len == sizeof(struct sockaddr_in)) {
                    memcpy(&gateway, addr, addr->sa_len);
                    return false;
                }
                if (!gateway.ss_len && addr->sa_family == AF_INET6 &&
                    addr->sa_len == sizeof(struct sockaddr_in6)) {
                    memcpy(&gateway, addr, addr->sa_len);
                }
                return true;
            });
        }
    }
    update(context, index, gateway.ss_len ? (const struct sockaddr *)&gateway : NULL);
}

// On success the cancel handler owns context. On failure neither callback runs.
nw_path_monitor_t netwatch_path_start(void *context, update_fn update, cancel_fn cancelled) {
    if (__builtin_available(iOS 12.0, *)) {
        nw_path_monitor_t monitor = nw_path_monitor_create();
        if (!monitor) return NULL;
        dispatch_queue_t queue = dispatch_queue_create("netwatch.path", DISPATCH_QUEUE_SERIAL);
        if (!queue) {
            nw_release(monitor);
            return NULL;
        }
        nw_path_monitor_set_queue(monitor, queue);
        nw_path_monitor_set_update_handler(monitor, ^(nw_path_t path) {
            report(path, context, update);
        });
        nw_path_monitor_set_cancel_handler(monitor, ^{
            cancelled(context); // last callback on the serial queue
        });
        nw_path_monitor_start(monitor);
        dispatch_release(queue); // the monitor retains it
        return monitor;
    }
    return NULL;
}

void netwatch_path_stop(nw_path_monitor_t monitor) {
    if (__builtin_available(iOS 12.0, *)) {
        nw_path_monitor_cancel(monitor);
        nw_release(monitor);
    }
}
