<?php

class SwitchBoardChatApi {
    protected $sbchaturl;
    public function __construct($sbchaturl)
    {
        $this->sbchaturl = $sbchaturl;
    }
    private function send_api_request($callcenterapitoken,$data){
        // Initialize cURL
        $ch = curl_init($this->sbchaturl);

        curl_setopt($ch, CURLOPT_POST, true);
        curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);

        // Comment when in prod !!
        curl_setopt($ch, CURLOPT_SSL_VERIFYPEER, false);
        curl_setopt($ch, CURLOPT_SSL_VERIFYHOST, false);


        // Add headers
        curl_setopt($ch, CURLOPT_HTTPHEADER, [
            "Authorization: Bearer $callcenterapitoken",
            "Content-Type: application/json",
            "Accept: application/json"
        ]);
        $dataencoded = json_encode($data);
        // Attach JSON body
        curl_setopt($ch, CURLOPT_POSTFIELDS, $dataencoded);

        // Execute
        $response = curl_exec($ch);
        $httpCode = curl_getinfo($ch, CURLINFO_HTTP_CODE);
        return ['dataencoded' => $dataencoded, 'response' => $response,'httpCode' => $httpCode];
    }
    public function add_new_call_center($callcenterapitoken,$voip_ms_username,$voip_ms_api_password,$call_center_did,$privilege_level=0,$renew_api_token=false){
        $data = [
            "command" => "add_new_call_center",
            "voip_ms_username" =>  $voip_ms_username,
            "voip_ms_api_password" =>  $voip_ms_api_password,
            "call_center_did" => $call_center_did,
            "privilege_level" =>  $privilege_level,
            "renew_api_token" => $renew_api_token
        ];
        $request = self::send_api_request($callcenterapitoken,$data);
        // Show result
        echo "Json sent: ".$request['dataencoded']. "\n";
        echo "HTTP Status:".$request['httpCode']."\n";
        echo "Response: ".$request['response']."\n";
        return json_decode($request['response'],true);

    }
    public function delete_call_center($callcenterapitoken,$callcenter_to_delete_api_token){
        $data = [
            "command" => "delete_call_center",
            "api_token" =>  $callcenter_to_delete_api_token
        ];
        $request = self::send_api_request($callcenterapitoken,$data);
        // Show result
        echo "Json sent: ".$request['dataencoded']. "\n";
        echo "HTTP Status:".$request['httpCode']."\n";
        echo "Response: ".$request['response']."\n";

    }
    public function change_call_center_respondents($callcenterapitoken,$respondent){
        $data = [
            "command" => "change_respondents",
            "api_token" => $callcenterapitoken,
            "respondent" =>  $respondent
        ];
        $request = self::send_api_request($callcenterapitoken,$data);
        
        // Show result
        echo "Json sent: ".$request['dataencoded']. "\n";
        echo "HTTP Status:".$request['httpCode']."\n";
        echo "Response: ".$request['response']."\n";

    }
     
     public function send_message_to_respondents($callcenterapitoken,$respondent,$message){
        $data = [
            "command" => "send_message",
            "respondent" =>  $respondent,
            "message" =>  $message
        ];
        $request = self::send_api_request($callcenterapitoken,$data);
        
        // Show result
        echo "Json sent: ".$request['dataencoded']. "\n";
        echo "HTTP Status:".$request['httpCode']."\n";
        echo "Response: ".$request['response']."\n";
        return json_decode($request['response'],true);

    }
    public function broadcast_message_to_all_respondents($callcenterapitoken,$message){
        $data = [
            "command" => "broadcast_message",
            "message" =>  $message
        ];
        $request = self::send_api_request($callcenterapitoken,$data);
        
        // Show result
        echo "Json sent: ".$request['dataencoded']. "\n";
        echo "HTTP Status:".$request['httpCode']."\n";
        echo "Response: ".$request['response']."\n";
        return json_decode($request['response'],true);

    }

    public function list_macros($callcenterapitoken){
        $data = [
            "command" => "list_macros",
            "api_token" => $callcenterapitoken,

        ];
        $request = self::send_api_request($callcenterapitoken,$data);
        
        // Show result
        echo "Json sent: ".$request['dataencoded']. "\n";
        echo "HTTP Status:".$request['httpCode']."\n";
        echo "Response: ".$request['response']."\n";
        $response_decoded = json_decode($request['response'],true);
        echo "Message : ".$response_decoded['message']."\n";
        $message_decoded = json_decode($response_decoded['message'],true);
        var_dump($message_decoded);
        if (count($message_decoded)>0){
            foreach($message_decoded as $message){
                var_dump($message);
              echo "Macro id {$message['id_macro']}\n"; 
              echo "Macro name {$message['macro_name']} Macro text : {$message['macro_text']} \n"; 
              if(!is_null($message['macro_tags']) ) {
                echo "Tags {$message['macro_tags']}\n";
                
              }else{
                echo "No tags\n";
              }
              echo "----------------------------\n";
            }

        }else{
        echo "No macros";
        }
        //var_dump($message_decoded);


    }
    
}
// API settings
$apiToken = getenv("SBCHAT_API_TOKEN"); // never hard-code it. A token was once committed in this file: rotate it (renew_api_token).
$url = "https://localhost:3443/sbchat_api";
$respondent = ["@alice:example.org","@bob:example.org","carol"];

$sbchatapi = new SwitchBoardChatApi($url);
//$sbchatapi->add_new_call_center($apiToken,"admin@example.org","PASSWORD",5145550102,0);
//$sbchatapi->add_new_call_center($apiToken,"admin@example.org","PASSWORD",0);
//$sbchatapi->add_new_call_center($apiToken,"test@example.org","PASSWORD",0,false);
//$sbchatapi->delete_call_center($apiToken,"TOKEN");
//$sbchatapi->change_call_center_respondents($apiToken,$respondent);
//$sbchatapi->send_message_to_respondents($apiToken,$respondent,"Ca fonctionne");
//$sbchatapi->broadcast_message_to_all_respondents($apiToken,"Bon 24h à tout le monde :)");
$sbchatapi->list_macros($apiToken);
// Data to send
/*$data = [
    "command" => "change_respondents",
    "respondent" =>  ["@alice:example.org","@bob:example.org","carol"]
];*/



?>