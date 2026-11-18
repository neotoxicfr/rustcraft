"""Minimal RCON client to drive the local test servers (teleports, commands).

Usage: python rcon.py <server-folder> "command 1" "command 2" ...
The port and password are read from the server's server.properties; a "sleep N" command waits
N seconds instead of being sent.
"""
import pathlib
import socket
import struct
import sys
import time


def props(folder):
    out = {}
    for line in (pathlib.Path(folder) / 'server.properties').read_text(encoding='utf-8').splitlines():
        if '=' in line and not line.startswith('#'):
            k, v = line.split('=', 1)
            out[k.strip()] = v.strip()
    return out


def packet(sock, req, kind, body):
    data = struct.pack('<ii', req, kind) + body.encode('utf-8') + b'\0\0'
    sock.sendall(struct.pack('<i', len(data)) + data)
    size = struct.unpack('<i', recv(sock, 4))[0]
    rid, _ = struct.unpack('<ii', recv(sock, 8))
    text = recv(sock, size - 8)[:-2].decode('utf-8', 'replace')
    return rid, text


def recv(sock, n):
    buf = b''
    while len(buf) < n:
        part = sock.recv(n - len(buf))
        if not part:
            raise ConnectionError('RCON closed')
        buf += part
    return buf


def main():
    folder, commands = sys.argv[1], sys.argv[2:]
    p = props(folder)
    with socket.create_connection(('127.0.0.1', int(p.get('rcon.port', '25575'))), timeout=10) as s:
        if packet(s, 1, 3, p['rcon.password'])[0] == -1:
            sys.exit('RCON password refused')
        for c in commands:
            if c.startswith('sleep '):
                time.sleep(float(c.split()[1]))
                continue
            print(f'> {c}\n{packet(s, 2, 2, c)[1]}')


if __name__ == '__main__':
    main()
