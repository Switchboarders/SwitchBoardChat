use crate::config::load_config;
pub struct Auth {
    homeserver_url: String,
    username: String,
    password: String,
}
impl Auth {
    pub fn new() -> Self {
        let config = load_config();
        Self {
            // To create the user using matrix authentification system (mas). Note that in the following command, mas is the container name
            // docker exec -it mas mas-cli manage register-user
            // (the command prints a compatibility token: never paste it here)
            homeserver_url: String::from(config.homeserver_url).clone(),
            username: config.matrix_username.clone(),
            password: config.matrix_password.clone(),
        }
    }
    pub fn get_homeserver_url(&self) -> String {
        self.homeserver_url.clone()
    }
    pub fn get_username(&self) -> String {
        self.username.clone()
    }
    pub fn get_password(&self) -> String {
        self.password.clone()
    }
}
