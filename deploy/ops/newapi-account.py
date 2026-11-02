#!/usr/bin/env python3
"""OWL authenticated member credentials and idempotent New API provisioning."""
import datetime, hashlib, hmac, json, os, re, secrets, sqlite3, string, threading, time, urllib.request
from zoneinfo import ZoneInfo
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from argon2 import PasswordHasher

OWL_DB = '/opt/service-tools/data/nai_gate.db'
NEWAPI_DB = '/opt/newapi/data/one-api.db'
OWL_BASE = 'http://127.0.0.1:3003'
SECRET = open('/opt/welfare/ai.secret').read().strip().encode()
INIT_QUOTA = int(os.environ.get('AI_INIT_QUOTA', '2500000'))
ALPHABET = 'abcdefghijkmnpqrstuvwxyz23456789'
PH = PasswordHasher(time_cost=2, memory_cost=19456, parallelism=1, hash_len=32, salt_len=16)
_lock = threading.Lock()


def derive_password(discord_id):
    raw = hmac.new(SECRET, discord_id.encode(), hashlib.sha256).digest()
    return ''.join(ALPHABET[b % len(ALPHABET)] for b in raw[:16])


def eligible_ids():
    # Active image members, plus authenticated OWL profiles with a recent
    # verification of one of the currently configured qualifying roles.
    with sqlite3.connect(f'file:{OWL_DB}?mode=ro', uri=True) as c:
        banned={r[0] for r in c.execute('SELECT discord_id FROM discord_bans')}
        ids={r[0] for r in c.execute('''SELECT DISTINCT k.discord_id FROM api_keys k
            JOIN discord_registrations r ON r.discord_id=k.discord_id AND r.key_id=k.id
            WHERE k.enabled=1 AND k.discord_id IS NOT NULL AND k.discord_id<>''
            AND (k.expires_at IS NULL OR k.expires_at=0 OR k.expires_at>?)''', (time.time(),))}
        setting=c.execute('SELECT value FROM site_settings WHERE key=?',('claim_rules',)).fetchone()
        rules=json.loads(setting[0]).get('roles',[]) if setting else []
        for rule in rules:
            ids.update(r[0] for r in c.execute('''SELECT p.discord_id FROM web_profiles p
                JOIN role_verifications v ON v.discord_id=p.discord_id
                WHERE v.guild_id=? AND v.role_id=? AND v.verified_at>?''',
                (rule['guild'],rule['role'],time.time()-30*86400)))
        return ids-banned


def discord_id_from_cookie(cookie):
    # Browser identifiers are not authentication. The OWL application verifies its
    # signed, unexpired member session and current key fingerprint on every call.
    req = urllib.request.Request(OWL_BASE+'/public/me', headers={'Cookie': cookie or ''})
    with urllib.request.urlopen(req, timeout=10) as r:
        me = json.load(r)
    uid = str((me.get('discord') or {}).get('id') or '')
    if not me.get('logged_in') or uid not in eligible_ids():
        return None
    return uid


def _ensure(uid, allowed_ids=None):
    if uid not in (eligible_ids() if allowed_ids is None else allowed_ids):
        raise PermissionError('not an eligible OWL member')
    now = int(time.time())
    with sqlite3.connect(NEWAPI_DB, timeout=20) as c:
        c.execute('BEGIN IMMEDIATE')
        row = c.execute('SELECT id,status,deleted_at FROM users WHERE username=?', (uid,)).fetchone()
        created = False
        if row is None:
            aff = secrets.token_hex(12)
            cur = c.execute('''INSERT INTO users
                (username,password,display_name,role,status,quota,used_quota,request_count,"group",
                 aff_code,aff_count,aff_quota,aff_history,inviter_id,created_at,last_login_at,auth_version)
                VALUES (?,?,?,1,1,?,0,0,'default',?,0,0,0,0,?,0,1)''',
                (uid, PH.hash(derive_password(uid)), uid, INIT_QUOTA, aff, now))
            user_id, created = cur.lastrowid, True
        else:
            user_id, status, deleted = row
            if status != 1 or deleted is not None:
                raise PermissionError('New API account disabled')
        row = c.execute('''SELECT "key" FROM tokens WHERE user_id=? AND status=1
            AND deleted_at IS NULL AND (expired_time=-1 OR expired_time>?)
            AND (unlimited_quota=1 OR remain_quota>0)
            AND ("group"='' OR "group"='default')
            AND (model_limits_enabled=0 OR model_limits_enabled IS NULL)
            ORDER BY CASE WHEN name='owl' THEN 0 ELSE 1 END,id LIMIT 1''', (user_id,now)).fetchone()
        token_created = False
        if row:
            key = row[0]
        else:
            key = ''.join(secrets.choice(string.ascii_letters+string.digits) for _ in range(48))
            c.execute('''INSERT INTO tokens
                (user_id,"key",status,name,created_time,accessed_time,expired_time,remain_quota,
                 unlimited_quota,model_limits_enabled,model_limits,allow_ips,used_quota,"group",
                 cross_group_retry,allow_user_quota_query)
                VALUES (?,?,1,'owl',?,0,-1,0,1,0,'','',0,'',0,1)''', (user_id,key,now))
            token_created = True
        return key, created, token_created


def ensure(uid):
    with _lock:
        return _ensure(uid)[0]


def reconcile():
    result = {'eligible':0,'created':0,'tokens_created':0,'disabled':0,'failed':0}
    ids = eligible_ids()
    result['eligible'] = len(ids)
    for uid in sorted(ids):
        try:
            with _lock:
                _, created, token_created = _ensure(uid, ids)
            result['created'] += int(created)
            result['tokens_created'] += int(token_created)
        except PermissionError:
            result['disabled'] += 1
        except Exception:
            result['failed'] += 1
    return result


def sync_loop():
    while True:
        try:
            result = reconcile()
            if result['created'] or result['tokens_created'] or result['failed']:
                print('reconcile',json.dumps(result),flush=True)
        except Exception:
            print('reconcile failed',flush=True)
        time.sleep(60)


def bot_context(uid):
    if uid not in eligible_ids():
        return {'eligible':False,'account_available':False}
    with sqlite3.connect(f'file:{NEWAPI_DB}?mode=ro',uri=True) as c:
        row=c.execute('SELECT id,status,quota FROM users WHERE username=? AND deleted_at IS NULL',(uid,)).fetchone()
        if not row: return {'eligible':True,'account_available':False}
        user_id,status,quota=row
        today=datetime.datetime.now(ZoneInfo('Asia/Shanghai')).strftime('%Y-%m-%d')
        checked=bool(c.execute('SELECT 1 FROM checkins WHERE user_id=? AND checkin_date=?',(user_id,today)).fetchone())
        models=set()
        for names,groups in c.execute('SELECT models,"group" FROM channels WHERE status=1'):
            if 'default' in (groups or '').split(','):models.update((names or '').split(','))
        settings=dict(c.execute('SELECT key,value FROM options WHERE key LIKE "checkin_setting.%"'))
        expr_row=c.execute('SELECT value FROM options WHERE key=?',('billing_setting.billing_expr',)).fetchone()
        expressions=json.loads(expr_row[0]) if expr_row else {}
        prices={}
        for name in models:
            expr=expressions.get(name,'')
            parsed={part:float(value) for part,value in re.findall(r'\b(p|c|cr|cc|cc1h)\s*\*\s*([0-9.]+)',expr)}
            if parsed: prices[name]={'usd_per_million_tokens':parsed}
    return {'eligible':True,'account_available':status==1,'balance_usd':round(quota/500000,6),
        'checked_in_today':checked,'checkin_enabled':settings.get('checkin_setting.enabled')=='true',
        'daily_reward_usd':[int(settings.get('checkin_setting.min_quota','0'))/500000,int(settings.get('checkin_setting.max_quota','0'))/500000],
        'checkin_url':'https://newapi.davidzhao.top/profile','available_models':sorted(models- {''}),'model_prices':prices,
        'server_time':int(time.time()),'timezone':'Asia/Shanghai','shared_pool_warning':'站内余额不代表上游套餐余量，模型列表代表已开放，不能单独证明上游实时生成成功'}


def bridge_secret():
    for line in open('/opt/service-tools/.env'):
        if line.startswith('REGISTRATION_BRIDGE_SECRET='):
            return line.split('=',1)[1].strip().strip('"').strip("'")
    return ''


class H(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *a):
        pass
    def _send(self, code, obj):
        b = json.dumps(obj, ensure_ascii=False).encode()
        self.send_response(code)
        self.send_header('content-type','application/json; charset=utf-8')
        self.send_header('cache-control','no-store')
        self.send_header('content-length',str(len(b)))
        self.send_header('x-content-type-options','nosniff')
        self.end_headers()
        self.wfile.write(b)
    def do_POST(self):
        if self.path != '/ai/api/context':
            return self._send(404,{'error':'not found'})
        expected=bridge_secret()
        actual=self.headers.get('Authorization','')
        if not expected or not hmac.compare_digest(actual,'Bearer '+expected):
            return self._send(401,{'error':'unauthorized'})
        try:
            size=int(self.headers.get('Content-Length','0'))
            if not 0 < size <= 1024: return self._send(400,{'error':'invalid body'})
            body=json.loads(self.rfile.read(size))
            uid=body.get('discord_id','')
            if not isinstance(uid,str) or not uid.isascii() or not uid.isdigit() or not 17 <= len(uid) <= 20:
                return self._send(400,{'error':'invalid member'})
            self._send(200,bot_context(uid))
        except Exception:
            self._send(503,{'error':'context unavailable'})
    def do_GET(self):
        if self.path.split('?')[0] != '/ai/api/account':
            return self._send(404,{'error':'not found'})
        try:
            uid = discord_id_from_cookie(self.headers.get('Cookie'))
            if not uid:
                return self._send(401,{'error':'请重新登录 owl 完成身份组验证'})
            key = ensure(uid)
            with sqlite3.connect(f'file:{NEWAPI_DB}?mode=ro',uri=True) as c:
                user_id,quota = c.execute('SELECT id,quota FROM users WHERE username=?',(uid,)).fetchone()
                today = datetime.datetime.now(ZoneInfo('Asia/Shanghai')).strftime('%Y-%m-%d')
                checked = bool(c.execute('SELECT 1 FROM checkins WHERE user_id=? AND checkin_date=?',(user_id,today)).fetchone())
            self._send(200,{'username':uid,'password':derive_password(uid),'key':'sk-'+key,
                'base':'https://newapi.davidzhao.top','checkin_url':'https://newapi.davidzhao.top/profile',
                'balance_usd':round(quota/500000,6),'checked_in_today':checked,
                'checkin_reward':'每日签到随机 $2–$5，每个账户每天限领一次；余额可累计'})
        except PermissionError:
            self._send(403,{'error':'账号暂不可用，请联系管理员'})
        except Exception:
            self._send(503,{'error':'账号服务暂不可用，请稍后重试'})

if __name__ == '__main__':
    threading.Thread(target=sync_loop, daemon=True).start()
    ThreadingHTTPServer(('127.0.0.1',3458),H).serve_forever()
