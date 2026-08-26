use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Exec {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    ReadFile {
        path: String,
    },
    WriteFile {
        path: String,
        content_base64: String,
    },
    ListDir {
        path: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Exec {
        stdout: String,
        stderr: String,
        exit_code: i32,
    },
    File {
        content_base64: String,
    },
    Dir {
        entries: Vec<String>,
    },
    Ok,
    Error {
        message: String,
    },
}
