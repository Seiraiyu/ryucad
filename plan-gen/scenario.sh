#!/bin/sh
# ryupdm 7.A's CLI scenario, in a folder of its own. $1: ryucad binary; $2: extra flag (--no-daemon or empty).
set -eu
B=$1; FLAG=${2:-}
T=$(mktemp -d /tmp/p7a/run.XXXXXX)
export RYUCAD_RUNTIME_DIR=$T/run RYUCAD_CONFIG_DIR=$T/config RYUCAD_CACHE_DIR=$T/cache
export GIT_CONFIG_GLOBAL=$T/gitconfig GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=T GIT_AUTHOR_EMAIL=t@example.com GIT_COMMITTER_NAME=T GIT_COMMITTER_EMAIL=t@example.com
export GIT_AUTHOR_DATE="2026-11-10T12:00:00Z" GIT_COMMITTER_DATE="2026-11-10T12:00:00Z"
mkdir -p $T/run $T/p && chmod 700 $T/run && cd $T/p
git init -q -b main
r() { $B $FLAG "$@"; }
r init >/dev/null
r part create bracket >/dev/null
r feature add bracket box --id b --param x=40 --param y=20 --param z=5 >/dev/null
r props set bracket number=100-0042 >/dev/null
r drawing create bracket d --views standard >/dev/null
r pdm configure --package-formats step,pdf,dxf,render >/dev/null
r release propose bracket >/dev/null
git add -A && git commit -q -m "release A"
echo "--- package A"
r release package bracket --out $T/a.zip | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['ok'], {k: d['result'][k] for k in ('label','file_name','key','hash','bytes')}); [print(' ', f['path'], f['size']) for f in d['result']['files']]"
r release package bracket --out $T/again.zip >/dev/null
cmp $T/a.zip $T/again.zip && echo "built twice: the same bytes"
sha256sum $T/a.zip | cut -c1-16
zipinfo $T/a.zip | head -12
python3 -I -c "import zipfile,sys; z=zipfile.ZipFile(sys.argv[1]); print('python zipfile:', z.testzip(), [i.filename for i in z.infolist()][:2])" $T/a.zip
echo "--- verify"
r release package verify $T/a.zip --rebuild | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['ok'], d['result']['verdict'], d['result']['rebuilt'], [w['code'] for w in d['warnings']] if d.get('warnings') else d['result'].get('warnings'))"
echo "--- tampered"
mkdir $T/x && cd $T/x && unzip -q $T/a.zip && printf '\n' >> step/100-0042-A.step && python3 -I -c "
import zipfile,os,sys
names=['package.json']+sorted(os.path.join(d,f)[2:] for d,_,fs in os.walk('.') for f in fs if os.path.join(d,f)[2:]!='package.json')
with zipfile.ZipFile(sys.argv[1],'w',zipfile.ZIP_DEFLATED) as z:
    [z.write(n) for n in names]" $T/bad.zip && cd $T/p
set +e
r release package verify $T/bad.zip; echo "exit $?"
r release package bracket --label B; echo "exit $?"
set -e
echo "--- release list"
r release list bracket | python3 -c "import json,sys; d=json.load(sys.stdin); print([ (x['label'], x.get('package')) for x in d['result']['records']])"
echo "--- folder"
r release package bracket --out $T/folder/ | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['ok'], d['result']['out'].endswith('folder'))"
ls -R $T/folder | head -20
echo "--- into the working folder"
cd $T && $B $FLAG -C $T/p release package bracket >/dev/null 2>&1 || true
r() { :; }
ls $T/p/*.zip $T/*.zip 2>/dev/null
$B -C $T/p daemon stop >/dev/null 2>&1 || true
echo "T=$T"
