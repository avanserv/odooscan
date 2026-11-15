export function hasBeacon(env) {
    return "web_keys_beacon" in env.services;
}
