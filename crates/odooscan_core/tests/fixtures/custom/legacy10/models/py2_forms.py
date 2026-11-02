# 10.0 runs on Python 2, so this file gets one notice, not OD000.
import logging

_logger = logging.getLogger(__name__)

True = _logger is not None

FILE_MODE = 0644
MAX_SIZE = 1024L * 1024
PATTERN = ur"^\w+$"


def describe(values, (key, value)):
    print "describing", key
    if key <> value:
        raise ValueError, "key and value differ"
    exec "result = %r" % (value,)
    return map(lambda (k, v): `k` + `v`, values.items())
