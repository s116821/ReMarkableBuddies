"""Owned host process fixture; no native code or actual broker interaction."""
import json
import os
import signal
import sys
import time

mode, broker, evidence, reader_directory = sys.argv[1:]
if mode == "exit":
    sys.exit(0)
if mode == "term-report":
    def terminate(_signal, _frame):
        with open(evidence + ".term", "x") as output:
            output.write("TERM")
        sys.exit(0)
    signal.signal(signal.SIGTERM, terminate)
elif mode == "ignore-term":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
# Readiness follows handler installation, never just process spawn.
with open(evidence, "x") as output:
    json.dump({"pid": os.getpid(), "argv": sys.argv, "cwd": os.getcwd(),
               "env": dict(os.environ), "nice": os.getpriority(os.PRIO_PROCESS, 0),
               "umask": os.umask(0o022)}, output)
if mode == "after-handshake":
    while not os.path.exists(os.path.join(reader_directory, "handshakes")):
        time.sleep(0.005)
if mode != "missing":
    with open(broker + ".pending", "x") as output:
        output.write(str(os.getpid()))
    os.rename(broker + ".pending", broker)
while True:
    time.sleep(1)
