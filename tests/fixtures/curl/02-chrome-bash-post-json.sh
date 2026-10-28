curl 'https://app.example.com/api/v2/projects' \
  -H 'accept: application/json' \
  -H 'content-type: application/json' \
  -H 'origin: https://app.example.com' \
  --data-raw '{"name":"Radiology worklist","tags":["mri","ct"],"active":true}'
