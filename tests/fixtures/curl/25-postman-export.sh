curl --location 'https://api.example.com/v1/orders?expand=items' \
--header 'Content-Type: application/json' \
--header 'Authorization: Bearer abc.def.ghi' \
--data '{
    "customer": "C-1001",
    "items": [
        {"sku": "XR-1", "qty": 2}
    ]
}'
