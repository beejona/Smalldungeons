# Checks the Rustclear mod handshake against the server directly (1.8.9, protocol 47), acting as
# the mod would: a client that never answers the hello is kicked after 10 s; a client that
# answers stays, can generate a dungeon with custom settings (status says the secret total and
# that it's unranked), and gets a reason back for settings no layout can meet.
# Usage: python3 tools/settings_test_18.py [port=4972]
import json, socket, struct, zlib, sys, time
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

def connect(name):
    global sock, rbuf, thr
    sock = socket.create_connection(('127.0.0.1', PORT)); sock.settimeout(30); rbuf = b''; thr = -1
    send(0x00, vi(47) + st('localhost') + struct.pack('>H', PORT) + vi(2))
    send(0x00, st(name))
    while True:
        pid, d = recv()
        if pid == 0x03: thr = rvi(d, 0)[0]
        elif pid == 0x02: return
        elif pid == 0x00: sys.exit('login refused: %r' % d[:200])

def payload(d):
    n, i = rvi(d, 0); channel = d[i:i + n].decode(); return channel, d[i + n:]

def mc_string(b):
    n, i = rvi(b, 0); return b[i:i + n].decode()

def settings(action, s=None):
    body = json.dumps({'version': 1, 'action': action, 'mod_version': 'test', 'settings': s or {}})
    send(0x17, st('rustclear:settings') + st(body))

def run(until, seconds, on_status=None):
    """Reads packets until `until(pid, data)` is true or time runs out. Returns what stopped it."""
    t0 = time.time()
    while time.time() - t0 < seconds:
        try: pid, d = recv()
        except (EOFError, ConnectionResetError): return 'closed'
        if pid == 0x00: send(0x00, d)
        elif pid == 0x46: globals()['thr'] = rvi(d, 0)[0]
        r = until(pid, d)
        if r: return r
    return 'timeout'

def fail(msg): print('FAIL', msg); sys.exit(1)

# 1. No mod: hello arrives, then a kick.
connect('nomod')
hello = {}
def no_mod(pid, d):
    if pid == 0x3F:
        ch, body = payload(d)
        if ch == 'rustclear:hello': hello.update(json.loads(mc_string(body)))
    if pid == 0x40: return 'kicked'
r = run(no_mod, 20)
if not hello: fail('no hello')
if r not in ('kicked', 'closed'): fail('client without the mod was not kicked: ' + r)
print('ok: hello has %d rooms, %d puzzles; client without the mod kicked' % (len(hello['rooms']), len(hello['puzzles'])))
sock.close()

# 2. With the mod.
connect('withmod')
statuses = []
def with_mod(want):
    def f(pid, d):
        if pid == 0x3F:
            ch, body = payload(d)
            if ch == 'rustclear:hello': settings('join')
            if ch == 'rustclear:status': statuses.append(json.loads(mc_string(body)))
        if pid == 0x40: return 'kicked'
        if len(statuses) >= want: return 'status'
    return f
if run(with_mod(1), 15) != 'status': fail('no status after join')
if run(with_mod(99), 12) == 'kicked': fail('client with the mod was kicked')
print('ok: client with the mod stays; ranked=%s' % statuses[0]['ranked'])

def generate(s, seconds=60):
    settings('generate', s)
    n = len(statuses)
    if run(with_mod(n + 1), seconds) != 'status': fail('no status after generate %r' % s)
    return statuses[-1]

# Custom settings: every status must be ok, unranked, and inside the secret range. The server log
# ("Rooms:" lines) shows the rooms each one placed; check it with LOG=path to the server output.
LOG = sys.argv[2] if len(sys.argv) > 2 else None
names = [r['name'] for r in hello['rooms']]
all_puzzles = [p['name'] for p in hello['puzzles']]
cases = [
    {'rooms': ['Old Trap'], 'puzzles': ['Boulder', 'Creeper Beams', 'Ice Fill', 'Three Weirdos', 'Water Board'], 'secrets': [30, 35]},
    {'rooms': ['Old Trap']},
    {'rooms': [n for n in names if n in ('Withermancer', 'Mural', 'Museum', 'Purple Flags')]},
    {'puzzles': all_puzzles[:2]},
    {'puzzles': all_puzzles[-5:]},
    {'secrets': [38, 40]},
    {'secrets': [66, 70]},
    {'secrets': [53, 53]},
]
for case in cases:
    s = generate(case)
    lo, hi = case.get('secrets', [0, 9999])
    if not s['ok'] or s['ranked'] or not lo <= s['total_secrets'] <= hi: fail('bad status for %r: %r' % (case, s))
    print('ok: %s -> %s' % (json.dumps(case), s['message']))

for case in [{'puzzles': ['Boulder']}, {'secrets': [0, 5]}, {'secrets': [200, 300]}, {'rooms': ['No Such Room']}]:
    s = generate(case, 30)
    if s['ok']: fail('impossible settings were accepted: %r' % case)
    print('ok: refused %s: %s' % (json.dumps(case), s['message']))

s = generate({})
if not s['ok'] or not s['ranked']: fail('fully random dungeon not ranked: %r' % s)
print('ok: fully random dungeon is ranked')

# Apply only stores the settings; they're used by the next generate.
settings('apply', {'rooms': ['Old Trap']})
n = len(statuses)
if run(with_mod(n + 1), 15) != 'status': fail('no status after apply')
if statuses[-1]['ranked'] is not True: fail('apply rebuilt the dungeon')
print('ok: apply stored settings: %s' % statuses[-1]['message'])
generate({})

if LOG:
    time.sleep(1)
    blocks = open(LOG).read().split('Dungeon String:')[1:]
    custom = [b for b in blocks if 'Settings:' in b]
    if len(custom) != len(cases): fail('expected %d custom dungeons in the log, found %d' % (len(cases), len(custom)))
    for case, block in zip(cases, custom):
        rooms = [r.strip() for r in block.split('Rooms:')[1].split('\n')[0].split(',')]
        for want in case.get('rooms', []):
            if want not in rooms: fail('%s missing from %r' % (want, rooms))
        if 'puzzles' in case:
            for r in rooms:
                if r in all_puzzles and r not in case['puzzles']: fail('puzzle %s not allowed in %r' % (r, case))
    print('ok: server log shows guaranteed rooms placed and only enabled puzzles')
print('PASS')
