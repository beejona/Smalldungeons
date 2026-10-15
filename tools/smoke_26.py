# Smoke test for the ViaProxy setup: a bare 26.1.x (protocol 775) client that logs in, gets
# through the configuration state into play, answers keep-alives and teleports, and counts what
# it receives. Usage: python3 tools/smoke_26.py [port=25565] [seconds=15]
# Exits 1 unless it reached play and received chunks. Packet ids are 26.1's (from ViaVersion's
# ClientboundPackets26_1 / ServerboundPackets26_1); unnamed ones print as hex.
import socket, struct, zlib, sys, time, uuid, collections
HOST, PORT, SECS = '127.0.0.1', int(sys.argv[1]) if len(sys.argv) > 1 else 25565, float(sys.argv[2]) if len(sys.argv) > 2 else 15
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
send(0x00, vi(775) + st(HOST) + struct.pack('>H', PORT) + vi(2))
send(0x00, st('beejtest') + uuid.uuid4().bytes)
state = 'login'; counts = collections.Counter(); hits = collections.Counter(); t0 = time.time()
CFG = {0x01:'custom_payload',0x02:'disconnect',0x03:'finish_cfg',0x04:'keep_alive',0x07:'registry_data',0x0C:'features',0x0D:'tags',0x0E:'known_packs'}
names = {0x0B: 'CHUNK_BATCH_FINISHED', 0x12: 'CONTAINER_SET_CONTENT', 0x14: 'CONTAINER_SET_SLOT', 0x20: 'DISCONNECT',
         0x2C: 'KEEP_ALIVE', 0x2D: 'LEVEL_CHUNK_WITH_LIGHT', 0x31: 'LOGIN', 0x48: 'PLAYER_POSITION', 0x79: 'SYSTEM_CHAT'}
try:
    while time.time() - t0 < SECS:
        pid, d = recv()
        if state == 'login':
            if pid == 0x03: thr = rvi(d, 0)[0]
            elif pid == 0x02: send(0x03); state = 'config'; print('login ok')
            elif pid == 0x00: print('login disconnect', d[:300]); break
            continue
        if state == 'config':
            counts['cfg:' + CFG.get(pid, hex(pid))] += 1
            if pid == 0x0E: send(0x07, vi(0))
            elif pid == 0x04: send(0x04, d[:8])
            elif pid == 0x03: send(0x03); state = 'play'; print('config ok')
            elif pid == 0x02: print('config disconnect', d[:300]); break
            continue
        nm = names.get(pid, hex(pid)); counts[nm] += 1
        for key in (b'ExtraAttributes', b'\x08\x00\x02id'):  # the compound, and string tags named "id"
            if key in d: hits[nm + ":" + repr(key)] += d.count(key)
        if nm == 'KEEP_ALIVE': send(0x1C, d[:8])
        elif nm == 'PLAYER_POSITION': send(0x00, vi(rvi(d, 0)[0])); send(0x2C)
        elif nm == 'CHUNK_BATCH_FINISHED': send(0x0B, struct.pack('>f', 20.0))
        elif nm == 'DISCONNECT': print('play disconnect', d[:300]); break
except (EOFError, socket.timeout) as e: print('closed', type(e).__name__)
print('state', state)
for k, v in sorted(counts.items(), key=lambda x: -x[1]): print(f'{v:6} {k}')
print('item-tag hits:', dict(hits))
sys.exit(0 if state == 'play' and counts['LEVEL_CHUNK_WITH_LIGHT'] else 1)
