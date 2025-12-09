use std::net::{TcpListener, TcpStream};
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::collections::HashMap;
use libc::{epoll_create1, epoll_ctl, epoll_wait, epoll_event, EPOLLIN, EPOLLERR, EPOLLHUP, EPOLL_CTL_ADD, EPOLL_CTL_DEL};
// Import Serde
use serde_derive::Deserialize;
use std::fs;

#[derive(Debug, Clone)]
struct HttpRequest {
    method: String,
    path: String,
    version: String,
    headers: HashMap<String, String>,
    #[allow(dead_code)]
    body: Vec<u8>,
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    status_text: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn new(status: u16, status_text: &str, body: &str) -> Self {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "text/html".to_string());
        headers.insert("Content-Length".to_string(), body.len().to_string());
        
        HttpResponse {
            status,
            status_text: status_text.to_string(),
            headers,
            body: body.as_bytes().to_vec(),
        }
    }
    
    fn to_bytes(&self) -> Vec<u8> {
        let mut response = format!("HTTP/1.1 {} {}\r\n", self.status, self.status_text);
        for (key, value) in &self.headers {
            response.push_str(&format!("{}: {}\r\n", key, value));
        }
        response.push_str("\r\n");
        
        let mut bytes = response.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

struct HttpParser;

impl HttpParser {
    fn parse(data: &[u8]) -> Option<HttpRequest> {
        let request_str = String::from_utf8_lossy(data);
        let lines: Vec<&str> = request_str.lines().collect();
        
        if lines.is_empty() {
            return None;
        }
        
        // Parse request line: "GET /path HTTP/1.1"
        let request_line_parts: Vec<&str> = lines[0].split_whitespace().collect();
        if request_line_parts.len() < 3 {
            return None;
        }
        
        let method = request_line_parts[0].to_string();
        let path = request_line_parts[1].to_string();
        let version = request_line_parts[2].to_string();
        
        // Parse headers
        let mut headers = HashMap::new();
        let mut body_start = 0;
        
        for (i, line) in lines.iter().enumerate().skip(1) {
            if line.is_empty() {
                body_start = i + 1;
                break;
            }
            
            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim().to_string();
                let value = line[colon_pos + 1..].trim().to_string();
                headers.insert(key, value);
            }
        }
        
        // Parse body
        let body = if body_start < lines.len() {
            lines[body_start..].join("\n").into_bytes()
        } else {
            Vec::new()
        };
        
        Some(HttpRequest {
            method,
            path,
            version,
            headers,
            body,
        })
    }
}

type RouteHandler = fn(&HttpRequest) -> HttpResponse;

struct Route {
    method: String,
    path: String,
    handler: RouteHandler,
}

struct Router {
    routes: Vec<Route>,
}

impl Router {
    fn new() -> Self {
        Router {
            routes: Vec::new(),
        }
    }
    
    fn register(&mut self, method: &str, path: &str, handler: RouteHandler) {
        self.routes.push(Route {
            method: method.to_string(),
            path: path.to_string(),
            handler,
        });
    }
    
    fn handle(&self, request: &HttpRequest) -> HttpResponse {
        // Try to find an exact match first
        for route in &self.routes {
            if route.method == request.method && route.path == request.path {
                return (route.handler)(request);
            }
        }
        
        // Try path prefix matching (for routes like /api/*)
        // But exclude root path "/" from prefix matching
        for route in &self.routes {
            if route.method == request.method && route.path != "/" && request.path.starts_with(&route.path) {
                return (route.handler)(request);
            }
        }
        
        // Default 404 response
        HttpResponse::new(404, "Not Found", &ErrorPages::not_found())
    }
}

// Error page builder
#[allow(dead_code)]
struct ErrorPages;

impl ErrorPages {
    #[allow(dead_code)]
    fn not_found() -> String {
        format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>404 Not Found</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            margin: 0;
            padding: 0;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            height: 100vh;
            display: flex;
            justify-content: center;
            align-items: center;
        }}
        .container {{
            text-align: center;
            background: white;
            padding: 50px;
            border-radius: 10px;
            box-shadow: 0 10px 40px rgba(0, 0, 0, 0.2);
            max-width: 600px;
        }}
        h1 {{
            color: #e74c3c;
            font-size: 72px;
            margin: 0;
            font-weight: 700;
        }}
        p {{
            color: #666;
            font-size: 18px;
            margin: 20px 0;
        }}
        a {{
            display: inline-block;
            margin-top: 20px;
            padding: 12px 30px;
            background: #667eea;
            color: white;
            text-decoration: none;
            border-radius: 5px;
            transition: background 0.3s;
        }}
        a:hover {{
            background: #764ba2;
        }}
        .error-details {{
            text-align: left;
            background: #f5f5f5;
            padding: 20px;
            border-radius: 5px;
            margin-top: 30px;
            font-size: 14px;
            color: #333;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>404</h1>
        <p>The page you're looking for doesn't exist.</p>
        <p>It might have been removed or the URL might be incorrect.</p>
        <a href="/">Go Home</a>
        <div class="error-details">
            <strong>Error Details:</strong><br>
            Resource not found on this server.
        </div>
    </div>
</body>
</html>"#
        )
    }

    #[allow(dead_code)]
    fn bad_request() -> String {
        format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>400 Bad Request</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            margin: 0;
            padding: 0;
            background: linear-gradient(135deg, #f093fb 0%, #f5576c 100%);
            height: 100vh;
            display: flex;
            justify-content: center;
            align-items: center;
        }}
        .container {{
            text-align: center;
            background: white;
            padding: 50px;
            border-radius: 10px;
            box-shadow: 0 10px 40px rgba(0, 0, 0, 0.2);
            max-width: 600px;
        }}
        h1 {{
            color: #f5576c;
            font-size: 72px;
            margin: 0;
            font-weight: 700;
        }}
        p {{
            color: #666;
            font-size: 18px;
            margin: 20px 0;
        }}
        a {{
            display: inline-block;
            margin-top: 20px;
            padding: 12px 30px;
            background: #f5576c;
            color: white;
            text-decoration: none;
            border-radius: 5px;
            transition: background 0.3s;
        }}
        a:hover {{
            background: #f093fb;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>400</h1>
        <p>Bad Request</p>
        <p>The server cannot process your request due to a client error.</p>
        <a href="/">Go Home</a>
    </div>
</body>
</html>"#
        )
    }

    #[allow(dead_code)]
    fn internal_error() -> String {
        format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>500 Internal Server Error</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            margin: 0;
            padding: 0;
            background: linear-gradient(135deg, #eb3349 0%, #f45c43 100%);
            height: 100vh;
            display: flex;
            justify-content: center;
            align-items: center;
        }}
        .container {{
            text-align: center;
            background: white;
            padding: 50px;
            border-radius: 10px;
            box-shadow: 0 10px 40px rgba(0, 0, 0, 0.2);
            max-width: 600px;
        }}
        h1 {{
            color: #eb3349;
            font-size: 72px;
            margin: 0;
            font-weight: 700;
        }}
        p {{
            color: #666;
            font-size: 18px;
            margin: 20px 0;
        }}
        a {{
            display: inline-block;
            margin-top: 20px;
            padding: 12px 30px;
            background: #eb3349;
            color: white;
            text-decoration: none;
            border-radius: 5px;
            transition: background 0.3s;
        }}
        a:hover {{
            background: #f45c43;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>500</h1>
        <p>Internal Server Error</p>
        <p>Something went wrong on our end. Please try again later.</p>
        <a href="/">Go Home</a>
    </div>
</body>
</html>"#
        )
    }

    #[allow(dead_code)]
    fn method_not_allowed() -> String {
        format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>405 Method Not Allowed</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            margin: 0;
            padding: 0;
            background: linear-gradient(135deg, #fa709a 0%, #fee140 100%);
            height: 100vh;
            display: flex;
            justify-content: center;
            align-items: center;
        }}
        .container {{
            text-align: center;
            background: white;
            padding: 50px;
            border-radius: 10px;
            box-shadow: 0 10px 40px rgba(0, 0, 0, 0.2);
            max-width: 600px;
        }}
        h1 {{
            color: #fa709a;
            font-size: 72px;
            margin: 0;
            font-weight: 700;
        }}
        p {{
            color: #666;
            font-size: 18px;
            margin: 20px 0;
        }}
        a {{
            display: inline-block;
            margin-top: 20px;
            padding: 12px 30px;
            background: #fa709a;
            color: white;
            text-decoration: none;
            border-radius: 5px;
            transition: background 0.3s;
        }}
        a:hover {{
            background: #fee140;
            color: #333;
        }}
    </style>
</head>
<body>
    <div class="container">
        <h1>405</h1>
        <p>Method Not Allowed</p>
        <p>The HTTP method used is not supported for this resource.</p>
        <a href="/">Go Home</a>
    </div>
</body>
</html>"#
        )
    }
}

// Route handlers
fn handle_root(_req: &HttpRequest) -> HttpResponse {
    HttpResponse::new(200, "OK", "<html><body><h1>Welcome to Localhost!</h1><p>Try visiting /api/users or /health</p></body></html>")
}

fn handle_health(_req: &HttpRequest) -> HttpResponse {
    HttpResponse::new(200, "OK", r#"{"status": "healthy"}"#)
}

fn handle_users(req: &HttpRequest) -> HttpResponse {
    let body = format!(
        r#"{{"path": "{}", "method": "{}"}}"#,
        req.path, req.method
    );
    HttpResponse::new(200, "OK", &body)
}

fn handle_api_catch_all(req: &HttpRequest) -> HttpResponse {
    let body = format!(
        r#"{{"message": "API endpoint", "path": "{}", "method": "{}"}}"#,
        req.path, req.method
    );
    HttpResponse::new(200, "OK", &body)
}

#[derive(Deserialize)]
struct Config {
    server: ServerConfig,
    #[allow(dead_code)]
    logging: LoggingConfig,
}

#[derive(Deserialize)]
struct ServerConfig {
    host: String,
    port: u16,
    timeout_ms: i32,
    max_events: usize,
}

#[derive(Deserialize)]
struct LoggingConfig {
    #[allow(dead_code)]
    level: String,
    #[allow(dead_code)]
    file: String,
}

#[allow(dead_code)]
#[derive(Debug)]
enum ServerError {
    Io(io::Error),
    Config(toml::de::Error),
    InvalidConfig(String),
}

impl From<io::Error> for ServerError {
    fn from(err: io::Error) -> ServerError {
        ServerError::Io(err)
    }
}

impl From<toml::de::Error> for ServerError {
    fn from(err: toml::de::Error) -> ServerError {
        ServerError::Config(err)
    }
}

struct Connection {
    stream: TcpStream,
    buffer: Vec<u8>,
    request: Option<HttpRequest>,
}

struct Server {
    listener: TcpListener,
    config: Config,
    epoll_fd: RawFd,
    connections: HashMap<RawFd, Connection>,
    router: Router,
}

impl Server {
    pub fn new(config_path: &str) -> io::Result<Server> {
        // Read and parse configuration
        let config_content = fs::read_to_string(config_path)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Failed to read config: {}", e)))?;
        
        let config: Config = toml::from_str(&config_content)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Failed to parse config: {}", e)))?;

        let address = format!("{}:{}", config.server.host, config.server.port);

        let listener = TcpListener::bind(&address)?;
        listener.set_nonblocking(true)?;
        
        // Create epoll instance
        let epoll_fd = unsafe { epoll_create1(0) };
        if epoll_fd < 0 {
            return Err(io::Error::last_os_error());
        }

        // Add listener to epoll
        let mut event = epoll_event {
            events: EPOLLIN as u32,
            u64: listener.as_raw_fd() as u64,
        };

        unsafe {
            if epoll_ctl(
                epoll_fd,
                EPOLL_CTL_ADD,
                listener.as_raw_fd(),
                &mut event as *mut epoll_event,
            ) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        
        println!("Server started on http://{}:{}/", config.server.host, config.server.port);
        
        // Initialize router with routes
        let mut router = Router::new();
        router.register("GET", "/", handle_root);
        router.register("GET", "/health", handle_health);
        router.register("GET", "/api/users", handle_users);
        router.register("POST", "/api/users", handle_users);
        router.register("GET", "/api/", handle_api_catch_all);
        router.register("POST", "/api/", handle_api_catch_all);
        
        Ok(Server {
            listener,
            config,
            epoll_fd,
            connections: HashMap::new(),
            router,
        })
    }
    
    pub fn run(&mut self) -> io::Result<()> {
        let mut events = vec![epoll_event { events: 0, u64: 0 }; self.config.server.max_events];
        
        loop {
            let num_events = unsafe {
                epoll_wait(
                    self.epoll_fd,
                    events.as_mut_ptr(),
                    self.config.server.max_events as i32,
                    self.config.server.timeout_ms,
                )
            };

            if num_events < 0 {
                return Err(io::Error::last_os_error());
            }

            for i in 0..num_events as usize {
                let fd = events[i].u64 as RawFd;

                if fd == self.listener.as_raw_fd() {
                    // Handle new connection
                    self.accept_connection()?;
                } else {
                    // Handle existing connection
                    if events[i].events & (EPOLLERR as u32 | EPOLLHUP as u32) != 0 {
                        self.remove_connection(fd)?;
                        continue;
                    }

                    if events[i].events & EPOLLIN as u32 != 0 {
                        if let Err(_) = self.handle_client_data(fd) {
                            self.remove_connection(fd)?;
                            continue;
                        }
                        
                        // Check if we have a complete request to respond to
                        if let Some(connection) = self.connections.get_mut(&fd) {
                            if let Some(request) = &connection.request {
                                // Route the request
                                let response = self.router.handle(request);
                                
                                // Send response
                                if let Err(_) = connection.stream.write_all(&response.to_bytes()) {
                                    self.remove_connection(fd)?;
                                } else {
                                    if let Err(_) = connection.stream.flush() {
                                        self.remove_connection(fd)?;
                                    } else {
                                        // Reset for potential next request
                                        connection.request = None;
                                        connection.buffer.clear();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn accept_connection(&mut self) -> io::Result<()> {
        match self.listener.accept() {
            Ok((stream, addr)) => {
                println!("New connection from: {}", addr);
                stream.set_nonblocking(true)?;
                
                let fd = stream.as_raw_fd();
                let mut event = epoll_event {
                    events: EPOLLIN as u32,
                    u64: fd as u64,
                };

                unsafe {
                    if epoll_ctl(self.epoll_fd, EPOLL_CTL_ADD, fd, &mut event as *mut epoll_event) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }

                self.connections.insert(fd, Connection {
                    stream,
                    buffer: Vec::with_capacity(4096),
                    request: None,
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => eprintln!("Error accepting connection: {}", e),
        }
        Ok(())
    }

    fn remove_connection(&mut self, fd: RawFd) -> io::Result<()> {
        unsafe {
            epoll_ctl(self.epoll_fd, EPOLL_CTL_DEL, fd, std::ptr::null_mut());
        }
        self.connections.remove(&fd);
        Ok(())
    }

    fn handle_client_data(&mut self, fd: RawFd) -> io::Result<()> {
        if let Some(connection) = self.connections.get_mut(&fd) {
            let mut buffer = [0; 4096];
            match connection.stream.read(&mut buffer) {
                Ok(0) => {
                    // Connection closed by client
                    println!("Connection closed by client");
                    return Err(io::Error::new(io::ErrorKind::Other, "Connection closed"));
                }
                Ok(n) => {
                    // Append new data to the connection buffer
                    connection.buffer.extend_from_slice(&buffer[..n]);
                    
                    // Try to parse the HTTP request
                    if connection.request.is_none() {
                        if let Some(request) = HttpParser::parse(&connection.buffer) {
                            connection.request = Some(request.clone());
                            println!("Parsed HTTP Request:");
                            println!("  Method: {}", request.method);
                            println!("  Path: {}", request.path);
                            println!("  Version: {}", request.version);
                            println!("  Headers:");
                            for (key, value) in &request.headers {
                                println!("    {}: {}", key, value);
                            }
                            return Ok(());
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("Error reading from client: {}", e);
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn reload_config(&mut self, config_path: &str) -> io::Result<()> {
        let config_content = fs::read_to_string(config_path)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Failed to read config: {}", e)))?;
        
        self.config = toml::from_str(&config_content)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Failed to parse config: {}", e)))?;
        
        println!("Configuration reloaded successfully");
        Ok(())
    }
}

impl Config {
    #[allow(dead_code)]
    fn validate(&self) -> Result<(), ServerError> {
        if self.server.port == 0 {
            return Err(ServerError::InvalidConfig("Port cannot be 0".into()));
        }
        if self.server.max_events == 0 {
            return Err(ServerError::InvalidConfig("max_events cannot be 0".into()));
        }
        if self.server.timeout_ms < 0 {
            return Err(ServerError::InvalidConfig("timeout_ms cannot be negative".into()));
        }
        Ok(())
    }
}

fn main() -> io::Result<()> {
    let mut server = Server::new("config.toml")?;
    server.run()
}