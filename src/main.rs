use std::net::{TcpListener, TcpStream};
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::collections::HashMap;
use libc::{epoll_create1, epoll_ctl, epoll_wait, epoll_event, EPOLLIN, EPOLLERR, EPOLLHUP, EPOLL_CTL_ADD, EPOLL_CTL_DEL};
// Import Serde
use serde_derive::Deserialize;
use std::fs;
use std::process::{Command, Stdio};
use std::env;

// Form data structures
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct FormFile {
    filename: String,
    content_type: String,
    data: Vec<u8>,
}

#[derive(Debug, Clone)]
struct HttpRequest {
    method: String,
    path: String,
    #[allow(dead_code)]
    query_string: Option<String>,
    version: String,
    headers: HashMap<String, String>,
    #[allow(dead_code)]
    cookies: HashMap<String, String>,
    #[allow(dead_code)]
    query_params: HashMap<String, String>,
    #[allow(dead_code)]
    form_fields: HashMap<String, String>,
    #[allow(dead_code)]
    form_files: HashMap<String, FormFile>,
    #[allow(dead_code)]
    body: Vec<u8>,
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    status_text: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    is_chunked: bool,
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
            is_chunked: false,
        }
    }
    
    fn to_bytes(&self) -> Vec<u8> {
        let mut response = format!("HTTP/1.1 {} {}\r\n", self.status, self.status_text);
        for (key, value) in &self.headers {
            response.push_str(&format!("{}: {}\r\n", key, value));
        }
        response.push_str("\r\n");
        
        let mut bytes = response.into_bytes();
        
        if self.is_chunked {
            // Encode body in chunks
            let chunk_size = 1024;
            for chunk in self.body.chunks(chunk_size) {
                let chunk_header = format!("{:x}\r\n", chunk.len());
                bytes.extend_from_slice(chunk_header.as_bytes());
                bytes.extend_from_slice(chunk);
                bytes.extend_from_slice(b"\r\n");
            }
            // Final chunk
            bytes.extend_from_slice(b"0\r\n\r\n");
        } else {
            bytes.extend_from_slice(&self.body);
        }
        
        bytes
    }
}

/// CGI Executor - Handles Common Gateway Interface script execution
struct CGIExecutor;

impl CGIExecutor {
    /// Execute a CGI script and return the HTTP response
    fn execute(
        script_path: &str,
        request: &HttpRequest,
        client_ip: &str,
    ) -> io::Result<HttpResponse> {
        // Verify script exists
        if !std::path::Path::new(script_path).exists() {
            return Ok(HttpResponse::new(404, "Not Found", "CGI script not found"));
        }

        // Make script executable
        std::process::Command::new("chmod")
            .arg("+x")
            .arg(script_path)
            .output()
            .ok();

        // Build environment variables for CGI
        let env_vars = Self::build_cgi_env(request, client_ip);

        // Determine request method for stdin handling
        let use_stdin = request.method == "POST" || request.method == "PUT";
        let stdin_data: &[u8] = if use_stdin { &request.body } else { &[] };

        // Execute the script
        let mut child = Command::new(script_path)
            .env_clear()
            .envs(&env_vars)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // Write request body to stdin if needed
        if use_stdin {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(stdin_data);
            }
        }

        // Wait for output with a simple approach: read all output synchronously
        // The subprocess should complete quickly for CGI scripts
        let output = child.wait_with_output()?;

        if !output.stderr.is_empty() {
            eprintln!("CGI stderr: {}", String::from_utf8_lossy(&output.stderr));
        }

        // Parse CGI response from bytes
        let output_str = String::from_utf8_lossy(&output.stdout);
        Self::parse_cgi_response(&output_str)
    }

    /// Build CGI environment variables based on HTTP request
    fn build_cgi_env(request: &HttpRequest, client_ip: &str) -> HashMap<String, String> {
        let mut env = HashMap::new();

        // CGI Standard Variables
        env.insert("REQUEST_METHOD".to_string(), request.method.clone());
        env.insert("SCRIPT_NAME".to_string(), request.path.clone());
        env.insert("PATH_INFO".to_string(), request.path.clone());
        env.insert("QUERY_STRING".to_string(), 
                   request.query_string.clone().unwrap_or_default());
        env.insert("CONTENT_LENGTH".to_string(), 
                   request.body.len().to_string());
        
        // HTTP Headers as environment variables
        if let Some(content_type) = request.headers.get("Content-Type") {
            env.insert("CONTENT_TYPE".to_string(), content_type.clone());
        } else {
            env.insert("CONTENT_TYPE".to_string(), "text/plain".to_string());
        }

        // Server information
        env.insert("SERVER_NAME".to_string(), "localhost".to_string());
        env.insert("SERVER_PORT".to_string(), "8000".to_string());
        env.insert("SERVER_PROTOCOL".to_string(), request.version.clone());
        env.insert("SERVER_SOFTWARE".to_string(), "localhost-http-server/1.0".to_string());

        // Client information
        env.insert("REMOTE_ADDR".to_string(), client_ip.to_string());
        env.insert("REMOTE_HOST".to_string(), client_ip.to_string());

        // HTTP Request Headers (converted to CGI format)
        for (key, value) in &request.headers {
            let cgi_key = format!("HTTP_{}", 
                key.to_uppercase().replace("-", "_"));
            env.insert(cgi_key, value.clone());
        }

        // Additional CGI variables
        env.insert("GATEWAY_INTERFACE".to_string(), "CGI/1.1".to_string());

        // Inherit system environment for PATH and other system variables
        for (key, value) in env::vars() {
            if key == "PATH" || key == "HOME" || key == "USER" {
                env.insert(key, value);
            }
        }

        env
    }

    /// Parse CGI response (headers + body)
    fn parse_cgi_response(output: &str) -> io::Result<HttpResponse> {
        // Split headers and body by double newline
        let parts: Vec<&str> = output.splitn(2, "\r\n\r\n").collect();
        
        let (headers_str, body_str) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            // Try with just \n\n
            let parts: Vec<&str> = output.splitn(2, "\n\n").collect();
            if parts.len() == 2 {
                (parts[0], parts[1])
            } else {
                // No headers, entire output is body
                ("Status: 200 OK", output)
            }
        };

        let mut status_code = 200u16;
        let mut status_text = "OK".to_string();
        let mut response_headers = HashMap::new();

        // Parse CGI headers
        for line in headers_str.lines() {
            if line.is_empty() {
                continue;
            }

            if line.starts_with("Status:") {
                let status_line = line.trim_start_matches("Status:").trim();
                let parts: Vec<&str> = status_line.splitn(2, ' ').collect();
                if parts.len() >= 1 {
                    if let Ok(code) = parts[0].parse::<u16>() {
                        status_code = code;
                        if parts.len() > 1 {
                            status_text = parts[1].to_string();
                        }
                    }
                }
            } else if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim();
                let value = line[colon_pos + 1..].trim();
                response_headers.insert(key.to_string(), value.to_string());
            }
        }

        // If no Content-Type was set, default to text/html
        if !response_headers.contains_key("Content-Type") {
            response_headers.insert("Content-Type".to_string(), "text/html".to_string());
        }

        Ok(HttpResponse {
            status: status_code,
            status_text,
            headers: response_headers,
            body: body_str.as_bytes().to_vec(),
            is_chunked: false,
        })
    }
}

/// Response Builder - Fluent API for constructing HTTP responses
struct ResponseBuilder {
    status: u16,
    status_text: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    cookies: Vec<(String, String)>, // (name, value) pairs
    is_chunked: bool,
}

impl ResponseBuilder {
    /// Create a new response builder
    fn new() -> Self {
        ResponseBuilder {
            status: 200,
            status_text: "OK".to_string(),
            headers: HashMap::new(),
            body: Vec::new(),
            cookies: Vec::new(),
            is_chunked: false,
        }
    }
    
    /// Set the HTTP status code
    fn status(mut self, status: u16, status_text: &str) -> Self {
        self.status = status;
        self.status_text = status_text.to_string();
        self
    }
    
    /// Add a response header
    fn header(mut self, key: &str, value: &str) -> Self {
        self.headers.insert(key.to_string(), value.to_string());
        self
    }
    
    /// Set Content-Type header
    fn content_type(mut self, content_type: &str) -> Self {
        self.headers.insert("Content-Type".to_string(), content_type.to_string());
        self
    }
    
    /// Set the response body as string
    fn body_text(mut self, body: &str) -> Self {
        self.body = body.as_bytes().to_vec();
        self
    }
    
    /// Set the response body as bytes
    #[allow(dead_code)]
    fn body_bytes(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }
    
    /// Add a Set-Cookie header
    #[allow(dead_code)]
    fn cookie(mut self, name: &str, value: &str) -> Self {
        self.cookies.push((name.to_string(), value.to_string()));
        self
    }
    
    /// Add a Set-Cookie with additional options
    fn cookie_with_options(mut self, name: &str, value: &str, max_age: Option<u32>, path: &str, http_only: bool) -> Self {
        let mut cookie_str = format!("{}={}", name, value);
        if let Some(age) = max_age {
            cookie_str.push_str(&format!("; Max-Age={}", age));
        }
        cookie_str.push_str(&format!("; Path={}", path));
        if http_only {
            cookie_str.push_str("; HttpOnly");
        }
        self.headers.insert(
            "Set-Cookie".to_string(),
            cookie_str,
        );
        self
    }
    
    /// Enable chunked transfer encoding
    fn chunked(mut self, enable: bool) -> Self {
        self.is_chunked = enable;
        if enable {
            self.headers.insert("Transfer-Encoding".to_string(), "chunked".to_string());
            // Remove Content-Length for chunked encoding
            self.headers.remove("Content-Length");
        }
        self
    }
    
    /// Serve a static file
    fn file(mut self, path: &str) -> Result<Self, std::io::Error> {
        let file_data = std::fs::read(path)?;
        let content_type = Self::get_content_type(path);
        
        self.body = file_data;
        self.headers.insert("Content-Type".to_string(), content_type);
        Ok(self)
    }
    
    /// Get content type based on file extension
    fn get_content_type(path: &str) -> String {
        let content_type = if path.ends_with(".html") {
            "text/html"
        } else if path.ends_with(".css") {
            "text/css"
        } else if path.ends_with(".js") {
            "application/javascript"
        } else if path.ends_with(".json") {
            "application/json"
        } else if path.ends_with(".png") {
            "image/png"
        } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
            "image/jpeg"
        } else if path.ends_with(".gif") {
            "image/gif"
        } else if path.ends_with(".svg") {
            "image/svg+xml"
        } else if path.ends_with(".pdf") {
            "application/pdf"
        } else if path.ends_with(".txt") {
            "text/plain"
        } else if path.ends_with(".xml") {
            "application/xml"
        } else if path.ends_with(".woff") {
            "font/woff"
        } else if path.ends_with(".woff2") {
            "font/woff2"
        } else {
            "application/octet-stream"
        };
        content_type.to_string()
    }
    
    /// Build the final HttpResponse
    fn build(mut self) -> HttpResponse {
        // Set Content-Length if not chunked and not already set
        if !self.is_chunked && !self.headers.contains_key("Content-Length") {
            self.headers.insert("Content-Length".to_string(), self.body.len().to_string());
        }
        
        // Add Set-Cookie headers for cookies added via cookie()
        for (name, value) in &self.cookies {
            self.headers.insert(
                "Set-Cookie".to_string(),
                format!("{}={}", name, value),
            );
        }
        
        HttpResponse {
            status: self.status,
            status_text: self.status_text,
            headers: self.headers,
            body: self.body,
            is_chunked: self.is_chunked,
        }
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
        
        // Parse request line: "GET /path?query=value HTTP/1.1"
        let request_line_parts: Vec<&str> = lines[0].split_whitespace().collect();
        if request_line_parts.len() < 3 {
            return None;
        }
        
        let method = request_line_parts[0].to_string();
        let full_path = request_line_parts[1];
        let version = request_line_parts[2].to_string();
        
        // Split path and query string
        let (path, query_string) = if let Some(pos) = full_path.find('?') {
            (
                full_path[..pos].to_string(),
                Some(full_path[pos + 1..].to_string()),
            )
        } else {
            (full_path.to_string(), None)
        };
        
        // Parse headers
        let mut headers = HashMap::new();
        let mut cookies = HashMap::new();
        let mut body_start = 0;
        let mut is_chunked = false;
        let mut content_type = String::new();
        
        for (i, line) in lines.iter().enumerate().skip(1) {
            if line.is_empty() {
                body_start = i + 1;
                break;
            }
            
            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim().to_string();
                let value = line[colon_pos + 1..].trim().to_string();
                
                // Special handling for Cookie header
                if key.to_lowercase() == "cookie" {
                    Self::parse_cookies(&value, &mut cookies);
                }
                
                // Check for chunked encoding
                if key.to_lowercase() == "transfer-encoding" {
                    is_chunked = value.to_lowercase().contains("chunked");
                }
                
                // Store content type for multipart parsing
                if key.to_lowercase() == "content-type" {
                    content_type = value.clone();
                }
                
                headers.insert(key, value);
            }
        }
        
        // Parse query parameters
        let query_params = if let Some(ref qs) = query_string {
            Self::parse_query_string(qs)
        } else {
            HashMap::new()
        };
        
        // Parse body
        let mut body = if body_start < lines.len() {
            lines[body_start..].join("\n").into_bytes()
        } else {
            Vec::new()
        };
        
        // Handle chunked encoding
        if is_chunked {
            body = Self::decode_chunked(&body);
        }
        
        // Parse form data (multipart or urlencoded)
        let (form_fields, form_files) = Self::parse_form_data(&content_type, &body);
        
        Some(HttpRequest {
            method,
            path,
            query_string,
            version,
            headers,
            cookies,
            query_params,
            form_fields,
            form_files,
            body,
        })
    }
    
    fn parse_cookies(cookie_header: &str, cookies: &mut HashMap<String, String>) {
        for cookie in cookie_header.split(';') {
            let cookie = cookie.trim();
            if let Some(pos) = cookie.find('=') {
                let name = cookie[..pos].trim().to_string();
                let value = cookie[pos + 1..].trim().to_string();
                cookies.insert(name, value);
            }
        }
    }
    
    fn parse_query_string(query_string: &str) -> HashMap<String, String> {
        let mut params = HashMap::new();
        for param in query_string.split('&') {
            if let Some(pos) = param.find('=') {
                let key = Self::url_decode(&param[..pos]);
                let value = Self::url_decode(&param[pos + 1..]);
                params.insert(key, value);
            } else {
                params.insert(Self::url_decode(param), String::new());
            }
        }
        params
    }
    
    fn url_decode(encoded: &str) -> String {
        let mut result = String::new();
        let mut bytes = encoded.bytes().peekable();
        
        while let Some(byte) = bytes.next() {
            match byte {
                b'%' => {
                    if let (Some(h1), Some(h2)) = (bytes.next(), bytes.next()) {
                        if let Ok(hex_str) = std::str::from_utf8(&[h1, h2]) {
                            if let Ok(byte_val) = u8::from_str_radix(hex_str, 16) {
                                result.push(byte_val as char);
                            }
                        }
                    }
                }
                b'+' => result.push(' '),
                b => result.push(b as char),
            }
        }
        
        result
    }
    
    fn decode_chunked(data: &[u8]) -> Vec<u8> {
        let mut result = Vec::new();
        let data_str = String::from_utf8_lossy(data);
        let lines: Vec<&str> = data_str.lines().collect();
        
        let mut i = 0;
        while i < lines.len() {
            let chunk_size_line = lines[i].trim();
            
            // Parse chunk size (hex number)
            if let Ok(chunk_size) = usize::from_str_radix(chunk_size_line, 16) {
                if chunk_size == 0 {
                    // Last chunk
                    break;
                }
                
                i += 1;
                if i < lines.len() {
                    let chunk_data = lines[i].as_bytes();
                    let data_to_add = std::cmp::min(chunk_size, chunk_data.len());
                    result.extend_from_slice(&chunk_data[..data_to_add]);
                }
            }
            
            i += 1;
        }
        
        result
    }
    
    fn parse_form_data(content_type: &str, body: &[u8]) -> (HashMap<String, String>, HashMap<String, FormFile>) {
        let mut fields = HashMap::new();
        let mut files = HashMap::new();
        
        if content_type.contains("application/x-www-form-urlencoded") {
            // Parse URL-encoded form data
            let body_str = String::from_utf8_lossy(body);
            for param in body_str.split('&') {
                if let Some(pos) = param.find('=') {
                    let key = Self::url_decode(&param[..pos]);
                    let value = Self::url_decode(&param[pos + 1..]);
                    fields.insert(key, value);
                }
            }
        } else if content_type.contains("multipart/form-data") {
            // Extract boundary from content type
            if let Some(boundary_start) = content_type.find("boundary=") {
                let boundary = &content_type[boundary_start + 9..];
                let boundary = if let Some(semicolon) = boundary.find(';') {
                    &boundary[..semicolon]
                } else {
                    boundary
                };
                
                Self::parse_multipart(body, boundary, &mut fields, &mut files);
            }
        }
        
        (fields, files)
    }
    
    fn parse_multipart(
        body: &[u8],
        boundary: &str,
        fields: &mut HashMap<String, String>,
        files: &mut HashMap<String, FormFile>,
    ) {
        let body_str = String::from_utf8_lossy(body);
        let boundary_marker = format!("--{}", boundary);
        let parts: Vec<&str> = body_str.split(&boundary_marker).collect();
        
        for part in parts.iter().skip(1) {
            if part.contains("--") {
                // End boundary
                break;
            }
            
            let part = part.trim();
            if let Some(blank_line_pos) = part.find("\r\n\r\n") {
                let headers_str = &part[..blank_line_pos];
                let content = &part[blank_line_pos + 4..];
                let content = content.trim_end_matches("\r\n");
                
                // Parse part headers
                let mut field_name = String::new();
                let mut filename = Option::<String>::None;
                let mut content_type_part = String::from("text/plain");
                
                for header_line in headers_str.lines() {
                    if let Some(colon_pos) = header_line.find(':') {
                        let header_name = header_line[..colon_pos].trim().to_lowercase();
                        let header_value = header_line[colon_pos + 1..].trim();
                        
                        if header_name == "content-disposition" {
                            // Parse: form-data; name="field_name"; filename="file.txt"
                            if let Some(name_start) = header_value.find("name=\"") {
                                let name_start = name_start + 6;
                                if let Some(name_end) = header_value[name_start..].find('"') {
                                    field_name = header_value[name_start..name_start + name_end].to_string();
                                }
                            }
                            
                            if let Some(file_start) = header_value.find("filename=\"") {
                                let file_start = file_start + 10;
                                if let Some(file_end) = header_value[file_start..].find('"') {
                                    filename = Some(header_value[file_start..file_start + file_end].to_string());
                                }
                            }
                        } else if header_name == "content-type" {
                            content_type_part = header_value.to_string();
                        }
                    }
                }
                
                // Store field or file
                if let Some(filename) = filename {
                    files.insert(
                        field_name,
                        FormFile {
                            filename,
                            content_type: content_type_part,
                            data: content.as_bytes().to_vec(),
                        },
                    );
                } else {
                    fields.insert(field_name, content.to_string());
                }
            }
        }
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
        // Check for CGI paths first (/cgi-bin/*)
        if request.path.starts_with("/cgi-bin/") {
            return handle_cgi(request, "127.0.0.1");
        }

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
    let html = r#"<!DOCTYPE html>
<html>
<head>
    <title>Localhost Server</title>
    <style>
        body { font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif; margin: 0; padding: 20px; background: linear-gradient(135deg, #667eea 0%, #764ba2 100%); min-height: 100vh; }
        .container { max-width: 900px; margin: 0 auto; background: white; padding: 40px; border-radius: 10px; box-shadow: 0 10px 40px rgba(0,0,0,0.2); }
        h1 { color: #667eea; margin-top: 0; }
        h2 { color: #764ba2; margin-top: 30px; }
        .endpoint { background: #f5f5f5; padding: 15px; margin: 10px 0; border-left: 4px solid #667eea; border-radius: 4px; }
        .endpoint code { background: #e8e8ff; padding: 2px 6px; border-radius: 3px; font-weight: bold; }
        .endpoint p { margin: 5px 0; color: #555; }
        a { color: #667eea; text-decoration: none; font-weight: bold; }
        a:hover { text-decoration: underline; }
    </style>
</head>
<body>
    <div class="container">
        <h1>🚀 Welcome to Localhost Server</h1>
        <p>A high-performance HTTP server built in Rust with epoll event loop and advanced HTTP parsing.</p>
        
        <h2>📍 Available Endpoints</h2>
        
        <div class="endpoint">
            <code>GET /health</code>
            <p>Check server health status in JSON format</p>
            <p><a href="/health">Visit /health</a></p>
        </div>
        
        <div class="endpoint">
            <code>GET /api/users</code>
            <p>Get user information with session management demonstration</p>
            <p><a href="/api/users">Visit /api/users</a></p>
        </div>
        
        <div class="endpoint">
            <code>GET /inspect</code>
            <p>Inspect HTTP request details (headers, cookies, query params, etc.)</p>
            <p><a href="/inspect">Visit /inspect</a></p>
        </div>
        
        <div class="endpoint">
            <code>GET/POST /form-test</code>
            <p>Test form parsing with URL-encoded and multipart/form-data support</p>
            <p><a href="/form-test">Visit /form-test</a></p>
        </div>
        
        <div class="endpoint">
            <code>GET /download</code>
            <p>Chunked transfer encoding demonstration for streaming responses</p>
            <p><a href="/download">Visit /download</a></p>
        </div>
        
        <div class="endpoint">
            <code>GET /login</code>
            <p>Session management demonstration with cookie options</p>
            <p><a href="/login">Visit /login</a></p>
        </div>
        
        <div class="endpoint">
            <code>GET /static</code>
            <p>Static file serving with automatic MIME type detection</p>
            <p><a href="/static">Visit /static</a></p>
        </div>
        
        <div class="endpoint">
            <code>GET /api/*</code>
            <p>Catch-all API endpoint for other paths</p>
            <p><a href="/api/posts">Example: /api/posts</a></p>
        </div>
        
        <h2>✨ Features</h2>
        <ul>
            <li>⚡ High-performance epoll-based event loop (Linux)</li>
            <li>🔍 Advanced HTTP parser with chunked encoding support</li>
            <li>📝 Form data parsing (URL-encoded and multipart)</li>
            <li>🍪 Cookie extraction and session management</li>
            <li>📊 Query parameter parsing with URL decoding</li>
            <li>📁 Static file serving with MIME type detection</li>
            <li>🔗 Fluent API response builder</li>
        </ul>
    </div>
</body>
</html>"#;
    
    ResponseBuilder::new()
        .status(200, "OK")
        .content_type("text/html; charset=utf-8")
        .body_text(html)
        .build()
}

fn handle_health(_req: &HttpRequest) -> HttpResponse {
    ResponseBuilder::new()
        .status(200, "OK")
        .content_type("application/json")
        .body_text(r#"{"status": "healthy", "timestamp": "2025-12-09T20:00:00Z"}"#)
        .header("Cache-Control", "no-cache")
        .build()
}

fn handle_users(req: &HttpRequest) -> HttpResponse {
    let body = format!(
        r#"{{"path": "{}", "method": "{}"}}"#,
        req.path, req.method
    );
    ResponseBuilder::new()
        .status(200, "OK")
        .content_type("application/json")
        .body_text(&body)
        .cookie_with_options("user_session", "session_12345", Some(3600), "/api", true)
        .build()
}

fn handle_api_catch_all(req: &HttpRequest) -> HttpResponse {
    let body = format!(
        r#"{{"message": "API endpoint", "path": "{}", "method": "{}", "timestamp": "2025-12-09T20:00:00Z"}}"#,
        req.path, req.method
    );
    ResponseBuilder::new()
        .status(200, "OK")
        .content_type("application/json")
        .body_text(&body)
        .header("X-API-Version", "1.0")
        .build()
}

fn handle_inspect(req: &HttpRequest) -> HttpResponse {
    let mut body = String::from(r#"<!DOCTYPE html>
<html>
<head>
    <title>Request Inspector</title>
    <style>
        body { font-family: monospace; margin: 20px; }
        .section { margin: 20px 0; padding: 10px; background: #f5f5f5; border-left: 4px solid #667eea; }
        h2 { color: #667eea; margin-top: 0; }
        table { width: 100%; border-collapse: collapse; }
        td { padding: 8px; border-bottom: 1px solid #ddd; }
        td:first-child { font-weight: bold; color: #333; width: 20%; }
        a { color: #667eea; text-decoration: none; }
        a:hover { text-decoration: underline; }
    </style>
</head>
<body>
    <h1>Request Inspector</h1>
"#);
    
    // Request line info
    body.push_str(&format!(
        r#"<div class="section">
        <h2>Request Line</h2>
        <table>
            <tr><td>Method:</td><td>{}</td></tr>
            <tr><td>Path:</td><td>{}</td></tr>
            <tr><td>Query String:</td><td>{}</td></tr>
            <tr><td>HTTP Version:</td><td>{}</td></tr>
        </table>
    </div>"#,
        req.method,
        req.path,
        req.query_string.as_ref().unwrap_or(&"(none)".to_string()),
        req.version
    ));
    
    // Headers
    if !req.headers.is_empty() {
        body.push_str(r#"<div class="section">
        <h2>Headers</h2>
        <table>"#);
        for (key, value) in &req.headers {
            body.push_str(&format!("<tr><td>{}:</td><td>{}</td></tr>", key, value));
        }
        body.push_str("</table></div>");
    }
    
    // Cookies
    if !req.cookies.is_empty() {
        body.push_str(r#"<div class="section">
        <h2>Cookies</h2>
        <table>"#);
        for (name, value) in &req.cookies {
            body.push_str(&format!("<tr><td>{}:</td><td>{}</td></tr>", name, value));
        }
        body.push_str("</table></div>");
    } else {
        body.push_str(r#"<div class="section">
        <h2>Cookies</h2>
        <p>(no cookies)</p>
    </div>"#);
    }
    
    // Query Parameters
    if !req.query_params.is_empty() {
        body.push_str(r#"<div class="section">
        <h2>Query Parameters</h2>
        <table>"#);
        for (key, value) in &req.query_params {
            body.push_str(&format!("<tr><td>{}:</td><td>{}</td></tr>", key, value));
        }
        body.push_str("</table></div>");
    } else {
        body.push_str(r#"<div class="section">
        <h2>Query Parameters</h2>
        <p>(no query parameters)</p>
    </div>"#);
    }
    
    body.push_str(r#"<div class="section">
        <h2>Test Links</h2>
        <p><a href="/inspect?name=John&age=30&city=NYC">With Query Params</a></p>
    </div>
    
    <div class="section">
        <h2>cURL Examples</h2>
        <p>Test with cookies:<br>
        <code>curl -b "session_id=abc123; user_id=42" http://localhost:8080/inspect</code></p>
        <p>Test with query params:<br>
        <code>curl "http://localhost:8080/inspect?key=value&name=test"</code></p>
    </div>
    
    </body>
</html>"#);
    
    ResponseBuilder::new()
        .status(200, "OK")
        .content_type("text/html; charset=utf-8")
        .body_text(&body)
        .header("X-Inspector", "true")
        .build()
}

fn handle_form_test(req: &HttpRequest) -> HttpResponse {
    let mut body = String::from(r#"<!DOCTYPE html>
<html>
<head>
    <title>Form Parser Test</title>
    <style>
        body { font-family: monospace; margin: 20px; background: #f5f5f5; }
        .container { max-width: 800px; margin: 0 auto; background: white; padding: 20px; border-radius: 8px; }
        h1 { color: #667eea; }
        .section { margin: 20px 0; padding: 15px; background: #f9f9f9; border-left: 4px solid #667eea; }
        h2 { color: #333; margin: 0 0 10px 0; }
        table { width: 100%; border-collapse: collapse; }
        td { padding: 8px; border-bottom: 1px solid #ddd; }
        td:first-child { font-weight: bold; width: 25%; color: #667eea; }
        code { background: #f0f0f0; padding: 2px 6px; border-radius: 3px; }
        .form-section { background: white; padding: 20px; margin: 20px 0; border: 1px solid #ddd; border-radius: 8px; }
        input, textarea { width: 100%; padding: 8px; margin: 5px 0; border: 1px solid #ddd; border-radius: 4px; font-family: monospace; }
        button { background: #667eea; color: white; padding: 10px 20px; border: none; border-radius: 4px; cursor: pointer; margin-top: 10px; }
        button:hover { background: #764ba2; }
    </style>
</head>
<body>
    <div class="container">
        <h1>HTTP Parser Features Demo</h1>"#);
    
    body.push_str(r#"
        <div class="section">
            <h2>✅ Implemented Features</h2>
            <ul>
                <li><strong>Parse Request Line:</strong> Method, Path, Query String, HTTP Version</li>
                <li><strong>Extract Cookies:</strong> Automatic parsing from Cookie header</li>
                <li><strong>Query Parameters:</strong> URL decoding support</li>
                <li><strong>Chunked Encoding:</strong> Automatic decoding of chunked transfer encoding</li>
                <li><strong>Form Data:</strong> URL-encoded and multipart/form-data parsing</li>
                <li><strong>Headers:</strong> All HTTP headers parsed and accessible</li>
            </ul>
        </div>"#);
    
    // Show current request details
    body.push_str(r#"
        <div class="section">
            <h2>Current Request Info</h2>
            <table>
                <tr><td>Method:</td><td>"#);
    body.push_str(&req.method);
    body.push_str(r#"</td></tr>
                <tr><td>Path:</td><td>"#);
    body.push_str(&req.path);
    body.push_str(r#"</td></tr>
                <tr><td>HTTP Version:</td><td>"#);
    body.push_str(&req.version);
    body.push_str(r#"</td></tr>
            </table>
        </div>"#);
    
    // Form fields
    if !req.form_fields.is_empty() {
        body.push_str(r#"
        <div class="section">
            <h2>Form Fields Parsed</h2>
            <table>"#);
        for (name, value) in &req.form_fields {
            body.push_str(&format!(
                r#"<tr><td>{}:</td><td><code>{}</code></td></tr>"#,
                name, value
            ));
        }
        body.push_str("</table></div>");
    }
    
    // Form files
    if !req.form_files.is_empty() {
        body.push_str(r#"
        <div class="section">
            <h2>Files Uploaded</h2>
            <table>"#);
        for (field_name, file) in &req.form_files {
            body.push_str(&format!(
                r#"<tr><td>{}:</td><td><code>{}</code> ({} bytes, type: {})</td></tr>"#,
                field_name, file.filename, file.data.len(), file.content_type
            ));
        }
        body.push_str("</table></div>");
    }
    
    // Test forms
    body.push_str(r#"
        <div class="form-section">
            <h2>📝 Test URL-Encoded Form</h2>
            <form method="POST" action="/form-test" enctype="application/x-www-form-urlencoded">
                <label>Username:</label><input type="text" name="username" value="john_doe">
                <label>Email:</label><input type="email" name="email" value="john@example.com">
                <label>Message:</label><textarea name="message">Hello from form!</textarea>
                <button type="submit">Submit Form (URL-Encoded)</button>
            </form>
        </div>
        
        <div class="form-section">
            <h2>📤 Test Multipart Form</h2>
            <form method="POST" action="/form-test" enctype="multipart/form-data">
                <label>Name:</label><input type="text" name="name" value="John Doe">
                <label>File:</label><input type="file" name="upload">
                <label>Description:</label><textarea name="description">File upload test</textarea>
                <button type="submit">Submit Form (Multipart)</button>
            </form>
        </div>
        
        <div class="section">
            <h2>cURL Examples</h2>
            <p><strong>URL-Encoded POST:</strong><br>
            <code>curl -X POST -d "username=john&email=john@example.com&message=hello" http://localhost:8080/form-test</code></p>
            <p><strong>With Chunked Encoding:</strong><br>
            <code>curl -X POST -H "Transfer-Encoding: chunked" -d "data=value" http://localhost:8080/form-test</code></p>
        </div>
    </div>
</body>
</html>"#);
    
    ResponseBuilder::new()
        .status(200, "OK")
        .content_type("text/html; charset=utf-8")
        .body_text(&body)
        .header("X-Form-Parser", "enabled")
        .build()
}

fn handle_download(_req: &HttpRequest) -> HttpResponse {
    // Demonstrate chunked transfer encoding for streaming responses
    let large_content = r#"<!DOCTYPE html>
<html>
<head>
    <title>Chunked Response Demo</title>
    <style>
        body { font-family: Arial, sans-serif; margin: 20px; padding: 20px; background: #f5f5f5; }
        .container { max-width: 800px; margin: 0 auto; background: white; padding: 20px; border-radius: 8px; }
        h1 { color: #667eea; }
        p { line-height: 1.6; color: #333; }
        .highlight { background: #fff3cd; padding: 10px; border-left: 4px solid #ffc107; margin: 10px 0; }
        code { background: #f0f0f0; padding: 2px 6px; border-radius: 3px; }
    </style>
</head>
<body>
    <div class="container">
        <h1>📥 Chunked Transfer Encoding Demo</h1>
        <p>This response was sent using <strong>Transfer-Encoding: chunked</strong>, allowing the server to stream large content without knowing the total size in advance.</p>
        
        <div class="highlight">
            <strong>✨ Benefits of Chunked Encoding:</strong>
            <ul>
                <li>Stream responses without pre-calculating Content-Length</li>
                <li>Ideal for dynamic or generated content</li>
                <li>Enable HTTP/1.1 trailers</li>
                <li>Support for gzip-like compression</li>
                <li>Better for real-time data (SSE, WebSocket upgrades)</li>
            </ul>
        </div>
        
        <h2>How It Works</h2>
        <p>With chunked encoding enabled, the response body is sent as a series of chunks:</p>
        <code>[chunk size in hex]\r\n[chunk data]\r\n[next chunk size]\r\n[chunk data]\r\n0\r\n</code>
        
        <p>The ResponseBuilder automatically handles this when you call <code>.chunked(true)</code>:</p>
        <code>ResponseBuilder::new().body_text("...").chunked(true).build()</code>
        
        <h2>Use Cases</h2>
        <ul>
            <li><strong>Server-Sent Events (SSE):</strong> Send real-time updates to clients</li>
            <li><strong>Large File Downloads:</strong> Stream without buffering entire file</li>
            <li><strong>API Responses:</strong> Generate large JSON responses on-the-fly</li>
            <li><strong>Web Sockets Upgrade:</strong> Handshake mechanism uses chunked encoding</li>
            <li><strong>Streaming Analytics:</strong> Send metrics as they're collected</li>
        </ul>
        
        <p><strong>View the HTTP headers:</strong> Open Developer Tools (F12) → Network tab and check the response headers for <code>Transfer-Encoding: chunked</code></p>
    </div>
</body>
</html>"#;
    
    ResponseBuilder::new()
        .status(200, "OK")
        .content_type("text/html; charset=utf-8")
        .body_text(large_content)
        .chunked(true)
        .header("Cache-Control", "no-store")
        .build()
}

fn handle_login(_req: &HttpRequest) -> HttpResponse {
    // Demonstrate advanced cookie management for sessions
    let html = r#"<!DOCTYPE html>
<html>
<head>
    <title>Session Management Demo</title>
    <style>
        body { font-family: Arial, sans-serif; margin: 20px; padding: 20px; background: linear-gradient(135deg, #667eea 0%, #764ba2 100%); min-height: 100vh; }
        .container { max-width: 600px; margin: 0 auto; background: white; padding: 30px; border-radius: 10px; box-shadow: 0 10px 40px rgba(0,0,0,0.2); }
        h1 { color: #667eea; margin-top: 0; }
        .session-box { background: #f0f7ff; padding: 15px; border: 2px solid #667eea; border-radius: 8px; margin: 20px 0; }
        .cookie-item { background: white; padding: 10px; margin: 10px 0; border-left: 4px solid #764ba2; }
        code { background: #f5f5f5; padding: 2px 6px; border-radius: 3px; font-weight: bold; }
        ul { line-height: 1.8; }
        .note { background: #fff3cd; padding: 10px; border-radius: 4px; margin: 10px 0; }
    </style>
</head>
<body>
    <div class="container">
        <h1>🔐 Session Management Demo</h1>
        
        <div class="session-box">
            <h2>Cookies Set by This Response:</h2>
            <div class="cookie-item">
                <strong>user_session:</strong> <code>session_12345</code><br>
                <small>HttpOnly, Max-Age: 3600 seconds (1 hour), Path: /</small>
            </div>
            <div class="cookie-item">
                <strong>preferences:</strong> <code>theme=dark&lang=en</code><br>
                <small>HttpOnly, Max-Age: 2592000 seconds (30 days), Path: /</small>
            </div>
        </div>
        
        <h2>💡 How SessionManagement Works</h2>
        <ul>
            <li><strong>HttpOnly Flag:</strong> Prevents JavaScript access, protects against XSS attacks</li>
            <li><strong>Max-Age:</strong> Session lifetime in seconds (3600 = 1 hour)</li>
            <li><strong>Path:</strong> Cookie scope (/ = entire domain)</li>
            <li><strong>Secure Flag:</strong> Should be set in production (HTTPS only)</li>
            <li><strong>SameSite:</strong> CSRF protection (not shown, but recommended)</li>
        </ul>
        
        <div class="note">
            <strong>✅ Recommended Practices:</strong>
            <ul>
                <li>Store sensitive data server-side, use session ID in cookie</li>
                <li>Always use HttpOnly flag for session cookies</li>
                <li>Use Secure flag in production (HTTPS only)</li>
                <li>Implement server-side session validation</li>
                <li>Set reasonable Max-Age values</li>
                <li>Implement logout to clear session cookies</li>
            </ul>
        </div>
        
        <h2>Implementation Example</h2>
        <p>Using <code>ResponseBuilder::cookie_with_options()</code>:</p>
        <code style="display: block; background: #f5f5f5; padding: 10px; border-radius: 4px; margin: 10px 0; overflow-x: auto;">
ResponseBuilder::new()<br>
&nbsp;&nbsp;.status(200, "OK")<br>
&nbsp;&nbsp;.cookie_with_options("user_session", "session_12345", Some(3600), "/", true)<br>
&nbsp;&nbsp;.body_text("...")<br>
&nbsp;&nbsp;.build()
        </code>
        
        <p>Next step: <a href="/protected" style="color: #667eea; font-weight: bold;">Visit /protected</a> to see session validation</p>
    </div>
</body>
</html>"#;
    
    ResponseBuilder::new()
        .status(200, "OK")
        .content_type("text/html; charset=utf-8")
        .body_text(html)
        .cookie_with_options("user_session", "session_12345", Some(3600), "/", true)
        .cookie_with_options("preferences", "theme=dark&lang=en", Some(2592000), "/", true)
        .header("X-Session-Demo", "true")
        .build()
}

fn handle_static(_req: &HttpRequest) -> HttpResponse {
    // Demonstrate static file serving with ResponseBuilder
    match ResponseBuilder::new().file("static/example.html") {
        Ok(builder) => {
            builder
                .status(200, "OK")
                .header("Cache-Control", "public, max-age=3600")
                .build()
        }
        Err(_) => {
            // If file not found, return 404 error page
            ResponseBuilder::new()
                .status(404, "Not Found")
                .content_type("text/html; charset=utf-8")
                .body_text(&ErrorPages::not_found())
                .build()
        }
    }
}

fn handle_cgi(req: &HttpRequest, client_ip: &str) -> HttpResponse {
    // Extract script name from path (e.g., /cgi-bin/script.cgi)
    let cgi_path = format!("cgi-bin/{}", req.path.trim_start_matches("/cgi-bin/"));
    
    match CGIExecutor::execute(&cgi_path, req, client_ip) {
        Ok(response) => response,
        Err(e) => {
            eprintln!("CGI execution error: {}", e);
            ResponseBuilder::new()
                .status(500, "Internal Server Error")
                .content_type("text/html; charset=utf-8")
                .body_text(&format!(
                    r#"<!DOCTYPE html>
<html>
<head>
    <title>CGI Error</title>
    <style>
        body {{ font-family: Arial, sans-serif; margin: 20px; background: #f5f5f5; }}
        .error {{ background: #ffebee; padding: 20px; border-left: 4px solid #f44336; border-radius: 4px; }}
        code {{ background: #f0f0f0; padding: 2px 6px; border-radius: 3px; }}
    </style>
</head>
<body>
    <div class="error">
        <h1>CGI Execution Error</h1>
        <p><strong>Error:</strong> <code>{}</code></p>
        <p>Failed to execute CGI script at <code>{}</code></p>
    </div>
</body>
</html>"#,
                    e, cgi_path
                ))
                .build()
        }
    }
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
        router.register("GET", "/inspect", handle_inspect);
        router.register("GET", "/form-test", handle_form_test);
        router.register("POST", "/form-test", handle_form_test);
        router.register("GET", "/api/users", handle_users);
        router.register("POST", "/api/users", handle_users);
        router.register("GET", "/download", handle_download);
        router.register("GET", "/login", handle_login);
        router.register("GET", "/static", handle_static);
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