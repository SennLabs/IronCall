curl -sS -o out.json -w '%{http_code}' --cert client.pem --proxy http://proxy.corp:3128 --retry 3 --frobnicate -v https://api.example.com/data
