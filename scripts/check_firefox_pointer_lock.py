#!/usr/bin/env python3
"""Check on a device that Portal Debug's Firefox still gets Portal's page fix
for pointer lock.

Firefox on Linux rejects requestPointerLock({unadjustedMovement: true}), which
drops games such as surfd.net out of fullscreen. Portal's Firefox config
(FIREFOX_CFG in src/core/guest_browser.rs) makes every page ask for the plain
lock instead. A Firefox update broke the earlier fix once (it removed the pref
Portal set), so run this after Firefox updates. It loads a page in a throwaway
headless Firefox in the live desktop and checks that the page and an iframe in
it see Portal's requestPointerLock. Needs no taps and leaves the user's
Firefox profile alone.

Exit status: 0 when both see it, 1 when either does not, 2 when Firefox did
not report (not installed, or did not start).
"""
import argparse
import base64
import json
import re
import sys

from guest_exec import Session

GUEST_SCRIPT = r'''
command -v firefox >/dev/null || { echo "no firefox"; exit 0; }
profile=$(mktemp -d /tmp/portal-ffcheck.XXXXXX) || exit 0
cat > "$profile/user.js" <<'EOF'
user_pref("devtools.console.stdout.content", true);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("browser.startup.homepage_override.mstone", "ignore");
EOF
cat > "$profile/check.html" <<'EOF'
<!doctype html><script>
const top_ = Element.prototype.requestPointerLock;
const frame = document.createElement("iframe");
frame.srcdoc = "<p>";
frame.onload = () => {
  const inner = frame.contentWindow.Element.prototype.requestPointerLock;
  console.log("PORTAL-CHECK " + btoa(JSON.stringify({
    page: /unadjustedMovement/.test(String(top_)),
    iframe: /unadjustedMovement/.test(String(inner)),
    userAgent: navigator.userAgent,
  })));
};
document.documentElement.append(frame);
</script>
EOF
timeout 60 firefox --headless --no-remote --profile "$profile" "file://$profile/check.html" \
    > "$profile/out.txt" 2>&1 &
pid=$!
for _ in $(seq 1 55); do
    grep -aq PORTAL-CHECK "$profile/out.txt" && break
    sleep 1
done
kill "$pid" 2>/dev/null
wait "$pid" 2>/dev/null
grep -a -o 'PORTAL-CHECK [A-Za-z0-9+/=]*' "$profile/out.txt" | head -1
rm -rf "$profile"
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--serial')
    args = parser.parse_args()
    session = Session.connect(args.serial)
    result = session.run('bash -s', input=GUEST_SCRIPT.encode(), capture_output=True, timeout=120)
    output = result.stdout.decode(errors='replace')
    match = re.search(r'PORTAL-CHECK ([A-Za-z0-9+/=]+)', output)
    if not match:
        print('Firefox did not report:', output.strip() or result.stderr.decode(errors='replace').strip())
        return 2
    report = json.loads(base64.b64decode(match.group(1)))
    print(json.dumps(report, indent=2))
    if report['page'] and report['iframe']:
        print('OK: pages get the plain pointer lock')
        return 0
    print('FAIL: Portal\'s pointer-lock fix is missing; check FIREFOX_CFG in src/core/guest_browser.rs '
          'against this Firefox')
    return 1


if __name__ == '__main__':
    sys.exit(main())
