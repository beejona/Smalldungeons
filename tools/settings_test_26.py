# The Rustclear mod handshake through ViaProxy, as a bare 26.1 client (protocol 775): with
# ANSWER=1 it answers the hello like the mod and asks for a custom dungeon; with ANSWER=0 it
# should be kicked. Usage: python3 tools/settings_test_26.py 0|1 [port=25565] [seconds=20]
import socket, struct, zlib, sys, time, uuid, collections
ANSWER = sys.argv[1] == '1'
NAME = 'mod26' if ANSWER else 'nomod26'
HOST, PORT, SECS = '127.0.0.1', int(sys.argv[2]) if len(sys.argv) > 2 else 25565, float(sys.argv[3]) if len(sys.argv) > 3 else 20
def vi(n):
    n &= 0xffffffff; o = b''
    while True:
        b = n & 0x7f; n >>= 7
        if n: o += bytes([b | 0x80])
        else: return o + bytes([b])
def rvi(buf, i):
    r = s = 0
    while True:
        b = buf[i]; i += 1; r |= (b & 0x7f) << s; s += 7
        if not b & 0x80: break
    if r & 0x80000000: r -= 1 << 32
    return r, i
def st(s): b = s.encode(); return vi(len(b)) + b
sock = socket.create_connection((HOST, PORT)); sock.settimeout(SECS + 5)
rbuf = b''; thr = -1
def recvn(n):
    global rbuf
    while len(rbuf) < n:
        d = sock.recv(65536)
        if not d: raise EOFError
        rbuf += d
    out, rbuf = rbuf[:n], rbuf[n:]; return out
def readvi_sock():
    r = s = 0
    while True:
        b = recvn(1)[0]; r |= (b & 0x7f) << s; s += 7
        if not b & 0x80: return r
def send(pid, data=b''):
    p = vi(pid) + data
    if thr >= 0:
        p = (vi(0) + p) if len(p) < thr else (vi(len(p)) + zlib.compress(p))
    sock.sendall(vi(len(p)) + p)
def recv():
    n = readvi_sock(); p = recvn(n)
    if thr >= 0:
        dl, i = rvi(p, 0); p = p[i:]
        if dl: p = zlib.decompress(p)
    pid, i = rvi(p, 0); return pid, p[i:]

import json
SB_CUSTOM_PAYLOAD = 0x16  # ServerboundPackets26_1.CUSTOM_PAYLOAD
def mcstr(d, i=0):
    n, i = rvi(d, i); return d[i:i + n].decode(errors='replace'), i + n
send(0x00, vi(775) + st(HOST) + struct.pack('>H', PORT) + vi(2))
send(0x00, st(NAME) + uuid.uuid4().bytes)
state = 'login'; t0 = time.time(); got = {}; result = 'timeout'; ka = None
try:
    while time.time() - t0 < SECS:
        pid, d = recv()
        if state == 'login':
            if pid == 0x03: thr = rvi(d, 0)[0]
            elif pid == 0x02: send(0x03); state = 'config'
            elif pid == 0x00: print('login disconnect', d[:300]); break
            continue
        if state == 'config':
            if pid == 0x0E: send(0x07, vi(0))
            elif pid == 0x04: send(0x04, d[:8])
            elif pid == 0x03: send(0x03); state = 'play'
            elif pid == 0x02: print('config disconnect', d[:300]); break
            continue
        if b'rustclear:' in d[:40]:
            ch, i = mcstr(d)
            if ch.startswith('rustclear:'):
                body, _ = mcstr(d, i); got[ch] = json.loads(body)
                print('got', ch, 'pid', hex(pid), body[:120])
                if ch == 'rustclear:hello' and ANSWER:
                    msg = json.dumps({'version': 1, 'action': 'join', 'mod_version': 'test26', 'settings': {}})
                    send(SB_CUSTOM_PAYLOAD, st('rustclear:settings') + st(msg))
                if ch == 'rustclear:status' and ANSWER and 'gen' not in got:
                    got['gen'] = 1
                    msg = json.dumps({'version': 1, 'action': 'generate', 'mod_version': 'test26', 'settings': {'rooms': ['Old Trap'], 'secrets': [40, 45]}})
                    send(SB_CUSTOM_PAYLOAD, st('rustclear:settings') + st(msg))
        if pid == 0x20 and state == 'play' and b'Rustclear' in d: result = 'kicked'; print('kicked:', d[:120]); break
        if pid == 0x2C: send(0x1C, d[:8])
        elif pid == 0x48: send(0x00, vi(rvi(d, 0)[0])); send(0x2C)
        elif pid == 0x0B: send(0x0B, struct.pack('>f', 20.0))
except (EOFError, socket.timeout, ConnectionResetError) as e: result = 'closed'; print('closed', type(e).__name__)
print('result', result, 'channels', sorted(got))
