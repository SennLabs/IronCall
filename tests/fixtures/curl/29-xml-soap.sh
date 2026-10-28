curl -X POST https://ws.example.com/soap -H 'Content-Type: text/xml; charset=utf-8' -H 'SOAPAction: "urn:GetStudy"' -d '<?xml version="1.0"?><Envelope><Body><GetStudy id="7"/></Body></Envelope>'
