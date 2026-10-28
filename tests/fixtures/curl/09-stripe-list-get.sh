curl -G https://api.stripe.com/v1/charges \
  -u "sk_test_4eC39HqLyjWDarjtT1zdp7dc:" \
  -d limit=3 \
  -d "created[gte]"=1700000000
