Api documentation :

The api user must authentificate using the following request : 

POST /api/data HTTP/1.1
Host: 127.0.0.1:3000
Authorization: Bearer my_secret_token
Content-Type: application/json
Accept: application/json

Example in php :
<?php
// API settings
$apiToken = "sbchatcallcentertoken";
$url = "https://sbchatserver.com:3443/sbchat_api";

// Data to send
$data = [
    "command" => "getIP"
];

// Initialize cURL
$ch = curl_init($url);

curl_setopt($ch, CURLOPT_POST, true);
curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);

// Add headers
curl_setopt($ch, CURLOPT_HTTPHEADER, [
    "Authorization: Bearer $apiToken",
    "Content-Type: application/json",
    "Accept: application/json"
]);

// Attach JSON body
curl_setopt($ch, CURLOPT_POSTFIELDS, json_encode($data));

// Execute
$response = curl_exec($ch);
$httpCode = curl_getinfo($ch, CURLINFO_HTTP_CODE);
curl_close($ch);

// Show result
echo "HTTP Status: $httpCode\n";
echo "Response: $response\n";

?>


Api commands :
Every command adressed to a switchboard chat server use the following json format :
{
  "command": "my_command",
  ...
}
The command parameter is forced, the other parameters varies in function of the command.

A json file is then returned in answer to the request.

/sbchat_api?get_IP
No authentification needed, return user ip address

/sbchat_api?
