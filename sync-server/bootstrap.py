"""Initialize persistent deployment state for Docker Compose."""
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import sqlite3
from contextlib import closing
from tempfile import NamedTemporaryFile
from urllib.parse import urlsplit


def write_private(path, text, uid=0):
    if not path.exists():
        stream = NamedTemporaryFile(mode='w', encoding='utf-8', dir=path.parent, delete=False)
        temporary = Path(stream.name)
        try:
            with stream:
                stream.write(text)
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)
    if os.name != 'nt':
        os.chown(path, uid, uid)
        os.chmod(path, 0o600)


def parse_origin(value):
    if not re.fullmatch(r'https://(?:[A-Za-z0-9.-]+|\[[0-9A-Fa-f:]+\])/?', value):
        raise ValueError('Use https://<DNS name or IP>, without a port or path.')
    parsed = urlsplit(value)
    host = parsed.hostname
    try:
        ipaddress.ip_address(host)
    except ValueError:
        labels = host.split('.')
        if (len(labels) < 2 or not re.search(r'[a-z]', labels[-1])
                or any(not re.fullmatch(r'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?', label)
                       for label in labels)):
            raise ValueError('Specify a fully qualified DNS name or an IP address.') from None
        is_ip = False
    else:
        is_ip = True
    return 'https://' + parsed.netloc.lower(), is_ip


def proxy_config(origin, is_ip, *, managed=False, registration=False):
    template = (Path(__file__).resolve().parent / 'Caddyfile.fragment').read_text(encoding='utf-8')
    tls = '''
    tls {
        issuer acme {
            dir https://acme-v02.api.letsencrypt.org/directory
            profile shortlived
        }
    }''' if is_ip else ''
    site = (template.replace('SYNC_ORIGIN', origin).replace('\n    SYNC_TLS', tls)
            .replace('SYNC_UPSTREAM', 'sync:47831' if managed else '127.0.0.1:47831')
            .replace('CASDOOR_UPSTREAM', 'casdoor:8000' if managed else '127.0.0.1:19000'))
    if not registration:
        site = site.replace('    @sync path', '    @signup path /signup /signup/* /api/signup /api/webauthn/signup/*\n    handle @signup {\n        respond 403\n    }\n    @sync path')
    if managed and is_ip:
        return '{\n    default_sni ' + urlsplit(origin).hostname + '\n}\n\n' + site
    return site


def bootstrap(state, env):
    setting = env.get("SUOJIAN_REGISTRATION", "false").lower()
    if setting not in ("true", "false"):
        raise ValueError("SUOJIAN_REGISTRATION must be true or false")
    registration = setting == "true"
    origin, is_ip = parse_origin(env.get('SUOJIAN_ORIGIN', ''))
    if origin != env['SUOJIAN_ORIGIN']:
        raise ValueError(f'Use the canonical origin {origin} in .env.')
    casdoor = state / 'casdoor-runtime'
    sync = state / 'sync-runtime'
    caddy = state / 'caddy-runtime'
    data = state / 'data'
    app_conf = casdoor / 'app.conf'
    init_data = casdoor / 'init_data.json'
    prior_conf = app_conf if app_conf.exists() else state / 'app.conf'
    prior_seed = init_data if init_data.exists() else state / 'init_data.json'
    config = prior_conf.read_text(encoding='utf-8') if prior_conf.exists() else None
    if config is not None and not re.search(r'^origin\s*=\s*' + re.escape(origin) + r'\s*$', config, re.M):
        raise ValueError('SUOJIAN_ORIGIN differs from the existing Casdoor deployment.')
    seed = json.loads(prior_seed.read_text(encoding='utf-8')) if prior_seed.exists() else None
    client_secret = env.get('CASDOOR_CLIENT_SECRET')
    if not client_secret and seed is not None:
        client_secret = next(app['clientSecret'] for app in seed['applications'] if app['clientId'] == 'suojian')
    for directory in (state, casdoor, sync, caddy, data):
        directory.mkdir(exist_ok=True)
    if os.name != 'nt':
        os.chown(data, 1000, 1000)
        os.chmod(data, 0o700)
    client_path = sync / 'client-secret'
    write_private(client_path, client_secret or secrets.token_urlsafe(48), uid=10001)
    write_private(casdoor / 'admin-password',
                  env.get('CASDOOR_ADMIN_PASSWORD') or secrets.token_urlsafe(32), uid=1000)
    account_path = state / 'initial-account.json'
    if not account_path.exists():
        write_private(account_path, json.dumps({'username': 'suojian', 'password': secrets.token_urlsafe(24)}))
    account = json.loads(account_path.read_text())
    organization = {'owner': 'admin', 'name': 'noteharbor', 'displayName': '闪记',
                    'websiteUrl': origin, 'passwordType': 'bcrypt', 'passwordOptions': ['AtLeast8'],
                    'defaultApplication': 'suojian', 'languages': ['zh', 'en'],
                    'accountItems': [{'name': name, 'visible': True, 'viewRule': 'Self', 'modifyRule': 'Self'}
                                     for name in ('Display name', 'Password', 'Email', '3rd-party logins')]}
    application = {'owner': 'admin', 'name': 'suojian', 'displayName': '闪记',
                   'organization': 'noteharbor', 'cert': 'cert-built-in',
                   'clientId': 'suojian', 'clientSecret': client_path.read_text(encoding='utf-8'),
                   'enablePassword': True, 'enableSignUp': False,
                   'signinMethods': [{'name': 'Password', 'displayName': '密码', 'rule': 'All'}],
                   'grantTypes': ['password', 'refresh_token', 'authorization_code'],
                   'tokenFormat': 'JWT-Standard', 'expireInHours': 0.25, 'refreshExpireInHours': 720,
                   'failedSigninLimit': 5, 'failedSigninFrozenTime': 15,
                   'redirectUris': [], 'providers': []}
    seed = seed if seed is not None else {
        'organizations': [organization], 'applications': [application],
        'users': [{'owner': 'noteharbor', 'name': account['username'],
                   'displayName': '闪记', 'type': 'normal-user', 'password': account['password']}],
    }
    application = next(a for a in seed['applications'] if a['clientId'] == 'suojian')
    application['enableSignUp'] = registration
    application['displayName'] = '闪记'
    for item in seed.get('organizations', []) + seed.get('users', []):
        if item.get('displayName') in ('索笺', 'NoteHarbor') and item.get('name') in ('noteharbor', 'suojian'):
            item['displayName'] = '闪记'
    items = [{'name': name, 'visible': name != 'ID', 'required': True,
              'rule': 'Random' if name == 'ID' else 'None'}
             for name in ('ID', 'Username', 'Password', 'Confirm password')]
    if not application.get('signupItems'):
        application['signupItems'] = items
    # initDataNewOnly deliberately preserves existing accounts. Update only our app's policy.
    db_path = data / 'casdoor.db'
    if db_path.exists():
        with closing(sqlite3.connect(db_path)) as db, db:
            db.execute("""UPDATE application SET display_name='闪记', enable_sign_up=?,
                signup_items=CASE WHEN signup_items IS NULL OR signup_items IN ('','null','[]')
                THEN ? ELSE signup_items END WHERE owner='admin' AND client_id='suojian'""",
                       (registration, json.dumps(items)))
            db.execute("UPDATE organization SET display_name='闪记' WHERE name='noteharbor' AND owner='admin' AND display_name IN ('索笺','NoteHarbor')")
            db.execute("UPDATE user SET display_name=replace(display_name,'索笺','闪记') WHERE owner='noteharbor' AND name='suojian' AND display_name IN ('索笺','索笺开发账号')")
    if config is not None:
        config = re.sub(r'^initDataFile\s*=.*$', 'initDataFile = /conf/init_data.json', config, flags=re.M)
    else:
        config = f'''appname = casdoor
httpport = 8000
runmode = prod
copyrequestbody = true
driverName = sqlite
dataSourceName = file:/data/casdoor.db?cache=shared
dbName = casdoor
showSql = false
origin = {origin}
originFrontend = {origin}
defaultLanguage = zh
defaultApplication = suojian
initDataFile = /conf/init_data.json
initDataNewOnly = true
frontendBaseDir = ./web/build
logConfig = {{"adapter":"console"}}
'''
    # Replace the seed atomically, preserving permissions and existing secrets.
    with NamedTemporaryFile(mode='w', encoding='utf-8', dir=casdoor, delete=False) as stream:
        json.dump(seed, stream, ensure_ascii=False, indent=2)
        temporary_seed = Path(stream.name)
    try:
        if os.name != 'nt':
            os.chown(temporary_seed, 1000, 1000)
        temporary_seed.replace(init_data)
    finally:
        temporary_seed.unlink(missing_ok=True)
    write_private(app_conf, config, uid=1000)
    (state / 'Caddyfile').write_text(proxy_config(origin, is_ip, registration=registration), encoding='utf-8')
    (caddy / 'Caddyfile').write_text(proxy_config(origin, is_ip, managed=True, registration=registration), encoding='utf-8')


if __name__ == '__main__':
    try:
        bootstrap(Path('/state'), os.environ)
    except ValueError as error:
        raise SystemExit(str(error)) from None
    print('Sync deployment configuration is ready.')
