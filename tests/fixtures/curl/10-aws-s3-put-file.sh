curl -X PUT 'https://my-bucket.s3.ap-southeast-2.amazonaws.com/scans/photo.jpg' \
  -H 'x-amz-date: 20261005T010203Z' \
  -H 'x-amz-content-sha256: UNSIGNED-PAYLOAD' \
  -H 'Authorization: AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261005/ap-southeast-2/s3/aws4_request, SignedHeaders=host;x-amz-date, Signature=abc123' \
  -H 'Content-Type: image/jpeg' \
  --data-binary @photo.jpg
