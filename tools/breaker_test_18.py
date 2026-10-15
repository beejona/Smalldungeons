# Checks the Dungeonbreaker end to end against the server directly (1.8.9, protocol 47):
# joins, swaps the breaker (inventory slot 17) into the hotbar, hits the block under its feet,
# and expects that block to turn to air, the lore to show 19 charges, and the block to come back
# about 11 s later. Then hits 21 more floor blocks at once and expects 20 to break and the last
# to be refused for lack of charges. Usage: python3 tools/breaker_test_18.py [port=4972]
import socket, struct, zlib, sys, time, uuid
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 4972
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
        if not b & 0x80: return r, i
def st(s): b = s.encode(); return vi(len(b)) + b
def pos_long(x, y, z): return struct.pack('>Q', ((x & 0x3FFFFFF) << 38) | ((y & 0xFFF) << 26) | (z & 0x3FFFFFF))
def unpos(v):
    x, y, z = v >> 38, (v >> 26) & 0xFFF, v & 0x3FFFFFF
    if x >= 1 << 25: x -= 1 << 26
    if z >= 1 << 25: z -= 1 << 26
    return x, y, z
sock = socket.create_connection(('127.0.0.1', PORT)); sock.settimeout(30)
rbuf = b''; thr = -1
def recvn(n):
    global rbuf
    while len(rbuf) < n:
        d = sock.recv(65536)
        if not d: raise EOFError
        rbuf += d
    out, rbuf = rbuf[:n], rbuf[n:]; return out
def recv():
    r = s = 0
    while True:
        b = recvn(1)[0]; r |= (b & 0x7f) << s; s += 7
        if not b & 0x80: break
    p = recvn(r)
    if thr >= 0:
        dl, i = rvi(p, 0); p = p[i:]
        if dl: p = zlib.decompress(p)
    pid, i = rvi(p, 0); return pid, p[i:]
def send(pid, data=b''):
    p = vi(pid) + data
    if thr >= 0: p = (vi(0) + p) if len(p) < thr else (vi(len(p)) + zlib.compress(p))
    sock.sendall(vi(len(p)) + p)
send(0x00, vi(47) + st('localhost') + struct.pack('>H', PORT) + vi(2))
send(0x00, st('breakertest'))
while True:
    pid, d = recv()
    if pid == 0x03: thr = rvi(d, 0)[0]
    elif pid == 0x02: break
    elif pid == 0x00: sys.exit('login refused: %r' % d[:200])
burst = []; burst_broken = set(); feet = None; target = None; events = []; t0 = time.time(); phase = 'join'; hit_at = None
def fail(msg): print('FAIL', msg); [print('  ', e) for e in events[-15:]]; sys.exit(1)
while time.time() - t0 < 40:
    pid, d = recv()
    now = time.time() - t0
    if pid == 0x00: send(0x00, d)                                   # keep alive
    elif pid == 0x46: thr = rvi(d, 0)[0]                            # set compression
    elif pid == 0x08 and feet is None:                              # position: stand there
        x, y, z = struct.unpack('>ddd', d[:24]); feet = (x, y, z)
        send(0x04, struct.pack('>ddd?', x, y, z, True))
    elif pid == 0x23:                                               # block change
        p = unpos(struct.unpack('>q', d[:8])[0]); state = rvi(d, 8)[0]
        if p == target: events.append((round(now, 2), 'block', state))
        if p in burst and state == 0: burst_broken.add(p)
    elif pid == 0x2F:                                               # set slot
        if b'Charges' in d:
            i = d.find(b'Charges: \xc2\xa7e') + 12; j = i
            while d[j:j + 1].isdigit(): j += 1
            events.append((round(now, 2), 'lore', d[i:j].decode()))
    elif pid == 0x02 and b'mystical' in d or pid == 0x02 and b'charges' in d:
        events.append((round(now, 2), 'chat', d[:120]))
    if phase == 'join' and feet and now > 3:
        target = (int(feet[0] // 1), int(feet[1] // 1) - 1, int(feet[2] // 1))
        send(0x16, vi(2))                                                          # open inventory
        send(0x0E, struct.pack('>bhbhb', 0, 17, 0, 1, 2) + struct.pack('>h', -1))  # swap slot 17 into hotbar 1
        send(0x0D, struct.pack('>b', 0))                                           # close it
        send(0x09, struct.pack('>h', 0))                                           # hold hotbar 1
        phase = 'swapped'
    elif phase == 'swapped' and now > 4:
        send(0x07, struct.pack('>b', 0) + pos_long(*target) + struct.pack('>b', 1))  # start digging, top face
        hit_at = now; phase = 'hit'
    elif phase == 'hit' and now > hit_at + 13:
        tx, ty, tz = target
        burst = [(tx + dx, ty, tz + dz) for dx in range(-2, 3) for dz in range(-2, 3)][:21]
        for b in burst:
            send(0x07, struct.pack('>b', 0) + pos_long(*b) + struct.pack('>b', 1))
        phase = 'burst'; burst_at = now
    elif phase == 'burst' and now > burst_at + 2:
        break
blocks = [e for e in events if e[1] == 'block']
lore = [e[2] for e in events if e[1] == 'lore']
print('target', target, 'events', events)
if not blocks or blocks[0][2] != 0: fail('the block did not turn to air')
if '19' not in lore: fail('the lore never showed 19 charges')
back = [e for e in blocks[1:] if e[2] != 0]
if not back: fail('the block never came back')
took = back[0][0] - blocks[0][0]
if not 10.0 <= took <= 12.5: fail('the block came back after %.2f s, expected about 11' % took)
print('PASS: broke, lore at 19, back after %.2f s' % took)
chats = [e for e in events if e[1] == 'chat']
print('burst: %d of %d broke' % (len(burst_broken), len(burst)), chats)
if len(burst_broken) != 20: fail('expected exactly 20 of the 21 burst hits to break')
if not any(b'enough charges' in c[2] for c in chats): fail('no "not enough charges" message after the 21st hit')
print('PASS: 20 of 21 broke, the 21st refused for lack of charges')
