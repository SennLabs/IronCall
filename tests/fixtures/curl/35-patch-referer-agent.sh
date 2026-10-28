curl -X PATCH https://api.example.com/items/42 -A 'IronCallTest/1.0' -e 'https://app.example.com/;auto' -H 'Content-Type: application/merge-patch+json' -d '{"qty":3}'
