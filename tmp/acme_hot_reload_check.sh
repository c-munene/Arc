#!/usr/bin/env bash
set -euo pipefail
cd /mnt/c/Users/Shuakami_Projects/CommunityProject/Arc
before=$(echo | openssl s_client -connect 127.0.0.1:18443 -servername localhost 2>/dev/null | openssl x509 -noout -serial | awk -F= '{print $2}')
openssl req -x509 -nodes -newkey rsa:2048 -keyout tmp/acme_smoke/static_key_new.pem -out tmp/acme_smoke/static_cert_new.pem -days 2 -subj '/CN=localhost' >/dev/null 2>&1
mv -f tmp/acme_smoke/static_cert_new.pem tmp/acme_smoke/static_cert.pem
mv -f tmp/acme_smoke/static_key_new.pem tmp/acme_smoke/static_key.pem
sleep 2
after=$(echo | openssl s_client -connect 127.0.0.1:18443 -servername localhost 2>/dev/null | openssl x509 -noout -serial | awk -F= '{print $2}')
echo "BEFORE=$before"
echo "AFTER=$after"
if [ "$before" != "$after" ]; then
  echo "HOT_RELOAD_CHANGED=YES"
else
  echo "HOT_RELOAD_CHANGED=NO"
fi
