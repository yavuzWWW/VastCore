"""Black-box Minecraft Java status and standalone process lifecycle checks."""
import argparse
import gzip
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import time

MAX_FRAME = 2_097_152
HOST = "127.0.0.1"


def varint(value):
    value &= 0xFFFFFFFF
    out = bytearray()
    while value >= 128:
        out.append((value & 127) | 128)
        value >>= 7
    out.append(value)
    return bytes(out)


def exact(stream, length):
    data = stream.read(length)
    if data is None or len(data) != length:
        raise EOFError("incomplete Minecraft packet")
    return data


def read_varint(stream):
    value = 0
    for i in range(5):
        byte = exact(stream, 1)[0]
        if i == 4 and byte & 0xF0:
            raise ValueError("invalid VarInt")
        value |= (byte & 127) << (7 * i)
        if not byte & 128:
            return value
    raise ValueError("unterminated VarInt")


def frame(payload):
    return varint(len(payload)) + payload


def read_frame(stream):
    from io import BytesIO
    length = read_varint(stream)
    if not 0 < length <= MAX_FRAME:
        raise ValueError(f"unexpected frame size {length}")
    return BytesIO(exact(stream, length))


def status_ping(port):
    with socket.create_connection((HOST, port), timeout=3) as sock:
        with sock.makefile("rb") as stream:
            address = HOST.encode("ascii")
            handshake = (
                b"\x00" + varint(-1) + varint(len(address)) + address
                + struct.pack(">H", port) + b"\x01"
            )
            sock.sendall(frame(handshake) + frame(b"\x00"))
            response = read_frame(stream)
            if read_varint(response) != 0:
                raise ValueError("wrong Java status response ID")
            size = read_varint(response)
            if size > MAX_FRAME:
                raise ValueError("oversized status JSON")
            status = json.loads(exact(response, size))
            if response.read(1):
                raise ValueError("trailing status response bytes")
            version = status["version"]
            if not isinstance(version["protocol"], int) or version["protocol"] <= 0:
                raise ValueError("invalid advertised protocol")
            if not version["name"] or "description" not in status or "players" not in status:
                raise ValueError("incomplete Java status")
            payload = struct.pack(">q", 0x123456789ABCDEF0)
            sock.sendall(frame(b"\x01" + payload))
            pong = read_frame(stream)
            if read_varint(pong) != 1 or pong.read() != payload:
                raise ValueError("Java ping payload mismatch")
            return version


def await_ready(process, port):
    deadline = time.monotonic() + 300
    last_error = None
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"server exited during startup: {process.returncode}")
        try:
            return status_ping(port)
        except (OSError, ValueError, EOFError, KeyError, TypeError) as err:
            last_error = err
            time.sleep(1)
    raise TimeoutError(f"Java status did not become ready: {last_error}")


def run_session(binary, directory, port, log_path):
    with log_path.open("wb") as log:
        process = subprocess.Popen(
            [str(binary)], cwd=directory, stdin=subprocess.PIPE,
            stdout=log, stderr=subprocess.STDOUT,
            env={**os.environ, "RUST_BACKTRACE": "1"},
        )
        try:
            version = await_ready(process, port)
            print(f"PASS status/ping: {version}", flush=True)
            process.stdin.write(b"stop\n")
            process.stdin.flush()
            if process.wait(timeout=180) != 0:
                raise RuntimeError(f"server exited with {process.returncode}")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=30)
            process.stdin.close()
    output = log_path.read_text(encoding="utf-8", errors="replace")
    if "panicked at" in output or "Failed to save level.dat" in output:
        raise RuntimeError(f"panic or world save error; see {log_path}")
    print(f"PASS graceful stop: {log_path.name}", flush=True)
    return version


def check_world(directory):
    level = directory / "world" / "level.dat"
    if not level.is_file():
        raise RuntimeError("level.dat missing after shutdown")
    with gzip.open(level, "rb") as stream:
        if stream.read(1) != b"\x0a":
            raise ValueError("level.dat does not start with an NBT compound")
        while stream.read(65536):
            pass
    print("PASS level.dat gzip integrity and NBT root", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--logs", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    if os.name == "nt" and binary.suffix != ".exe":
        binary = binary.with_suffix(".exe")
    logs = args.logs.resolve()
    logs.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="vastcore-smoke-") as tmp:
        directory = Path(tmp)
        with socket.socket() as sock:
            sock.bind((HOST, 0))
            port = sock.getsockname()[1]
        (directory / "pumpkin.toml").write_text(
            f"""default_level_name = "world"
allow_nether = false
allow_end = false
allow_chat_reports = false
[networking.java]
enabled = true
address = "{HOST}:{port}"
view_distance = 4
simulation_distance = 4
[networking.bedrock]
enabled = false
[networking.query]
enabled = false
[networking.management]
enabled = false
[networking.lan_broadcast]
enabled = false
[telemetry]
enabled = false
[update_checker]
enabled = false
""", encoding="utf-8",
        )
        first = run_session(binary, directory, port, logs / "first-start.log")
        check_world(directory)
        second = run_session(binary, directory, port, logs / "restart.log")
        check_world(directory)
        if first != second:
            raise RuntimeError("advertised version changed across restart")
        print("PASS restart and independent Java status compatibility", flush=True)


if __name__ == "__main__":
    main()
