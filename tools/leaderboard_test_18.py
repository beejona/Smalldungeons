# Checks the leaderboard protocol against the server directly (1.8.9, protocol 47), acting as the
# mod would. Start the server in a directory holding tools/leaderboard_fixture.json copied to
# leaderboard.json. It checks the channel is registered, a board comes back ranked fastest first
# with one entry per party, the requester's personal best and the current run's ranked state,
# and that custom settings make the current run unranked.
# Usage: python3 tools/leaderboard_test_18.py [port=4972]
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

def board_request(category=None, version=1):
    body = {'version': version}
    if category is not None: body['category'] = category
    send(0x17, st('rustclear:board') + st(json.dumps(body)))

connect('lbtest')
registered = []; boards = []; statuses = []; chats = []
def listen(want_boards=0, want_status=0):
    def f(pid, d):
        if pid == 0x3F:
            ch, body = payload(d)
            if ch == 'REGISTER': registered.extend(body.decode().split('\0'))
            if ch == 'rustclear:hello': settings('join')
            if ch == 'rustclear:status': statuses.append(json.loads(mc_string(body)))
            if ch == 'rustclear:board': boards.append(json.loads(mc_string(body)))
        if pid == 0x02: chats.append(d.decode(errors='replace'))
        if pid == 0x40: return 'kicked'
        if len(boards) >= want_boards and len(statuses) >= want_status: return 'done'
    return f
if run(listen(0, 1), 15) != 'done': fail('no status after join')
if 'rustclear:board' not in registered: fail('leaderboard channel not registered: %r' % registered)
print('ok: registered %r' % registered)

board_request('solo')
if run(listen(1, 1), 10) != 'done': fail('no board for solo')
b = boards[-1]
got = [(e['rank'], e['players'], e['ticks']) for e in b['entries']]
want = [(1, ['alice'], 2980), (2, ['bob'], 3100), (3, ['lbtest'], 3600)]
if got != want: fail('solo board %r, expected %r' % (got, want))
if b['personal_best'] is None or b['personal_best']['rank'] != 3: fail('personal best %r' % b['personal_best'])
if b['categories'] != ['solo', '2 players']: fail('categories %r' % b['categories'])
if not b['current_run_ranked'] or b['current_run_reason'] is not None: fail('fresh random dungeon should be ranked: %r' % b)
print('ok: solo board %r, personal best #3, current run ranked' % got)

board_request()
if run(listen(2, 1), 10) != 'done': fail('no board for the default category')
if boards[-1]['category'] != 'solo': fail('default category for one player is %r' % boards[-1]['category'])
board_request('2 players')
if run(listen(3, 1), 10) != 'done': fail('no board for 2 players')
if [e['players'] for e in boards[-1]['entries']] != [['alice', 'bob']] or boards[-1]['personal_best'] is not None: fail('2 players board %r' % boards[-1])
print('ok: default category solo; 2 players board has one party')

settings('generate', {'secrets': [30, 40]})
if run(listen(3, 2), 60) != 'done': fail('no status after generate')
board_request('solo')
if run(listen(4, 2), 10) != 'done': fail('no board after generate')
if boards[-1]['current_run_ranked'] or boards[-1]['current_run_reason'] != 'custom dungeon settings': fail('custom dungeon should be unranked: %r' % boards[-1])
print('ok: custom settings -> unranked (%s)' % boards[-1]['current_run_reason'])

board_request('solo', version=99)
run(lambda pid, d: (pid == 0x02 and b'leaderboard speaks version' in d) and 'chat', 10) == 'chat' or fail('no message for a wrong version')
print('ok: wrong version gets a message')
print('PASS')
