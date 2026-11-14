import { useService } from "@web/core/utils/hooks";

// Reported: only a test script probes for the service, and no shipped one.
export function useBeacon() {
    return useService("test_scripts18_beacon");
}
