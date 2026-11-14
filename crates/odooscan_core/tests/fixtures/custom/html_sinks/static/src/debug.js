export function trace(x, logger) {
    console.log(x);
    debugger;
    console.warn(x);
    logger.log(x);
    // eslint-disable-next-line no-console
    console.log(x);
    console.log(x); // eslint-disable-line no-console -- kept on purpose
    /* eslint-disable-next-line no-debugger */
    debugger;
    debugger; // eslint-disable-line no-debugger
    // eslint-disable-next-line no-debugger
    console.log(x);
    console.log(x); // eslint-disable-line
    /* eslint-disable no-console */
    console.log(x);
    /* eslint-enable no-console */
    console.log(x);
}
