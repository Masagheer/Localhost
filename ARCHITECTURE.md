# ResponseBuilder Architecture

## Class Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                      ResponseBuilder                             │
├─────────────────────────────────────────────────────────────────┤
│ Properties:                                                       │
│  - status: u16                                                   │
│  - status_text: String                                          │
│  - headers: HashMap<String, String>                            │
│  - body: Vec<u8>                                               │
│  - cookies: Vec<(String, String)>                              │
│  - is_chunked: bool                                            │
├─────────────────────────────────────────────────────────────────┤
│ Public Methods (Fluent API):                                    │
│  + new() → Self                                                 │
│  + status(u16, &str) → Self                                    │
│  + header(&str, &str) → Self                                  │
│  + content_type(&str) → Self                                  │
│  + body_text(&str) → Self                                     │
│  + body_bytes(Vec<u8>) → Self                                 │
│  + cookie(&str, &str) → Self                                 │
│  + cookie_with_options(...) → Self                           │
│  + chunked(bool) → Self                                       │
│  + file(&str) → Result<Self, Error>                          │
│  + get_content_type(&str) → String                           │
│  + build() → HttpResponse                                     │
└─────────────────────────────────────────────────────────────────┘
            │
            │ builds into
            ▼
┌─────────────────────────────────────────────────────────────────┐
│                      HttpResponse                                │
├─────────────────────────────────────────────────────────────────┤
│ - status: u16                                                   │
│ - status_text: String                                          │
│ - headers: HashMap<String, String>                            │
│ - body: Vec<u8>                                               │
│ - is_chunked: bool                                            │
├─────────────────────────────────────────────────────────────────┤
│ - to_bytes() → Vec<u8>  (with chunked encoding support)       │
└─────────────────────────────────────────────────────────────────┘
```

## Method Chaining Flow

```
ResponseBuilder::new()
        ↓
    .status(200, "OK")
        ↓
    .content_type("application/json")
        ↓
    .body_text(json_string)
        ↓
    .header("Cache-Control", "no-cache")
        ↓
    .build()
        ↓
    HttpResponse (ready to send to client)
```

## Request → Handler → ResponseBuilder → Response Flow

```
┌──────────────┐
│ HTTP Request │
└──────┬───────┘
       │
       ▼
┌─────────────────────────────┐
│ Router::handle(request)     │
│ - Match path                │
│ - Match method              │
│ - Call handler function     │
└──────┬──────────────────────┘
       │
       ▼
┌─────────────────────────────┐
│ Handler Function            │
│ fn handle_*(req) →          │
│    HttpResponse             │
└──────┬──────────────────────┘
       │
       ▼
┌─────────────────────────────────────┐
│ ResponseBuilder Pattern             │
│                                     │
│ ResponseBuilder::new()              │
│   .status(...)                      │
│   .header(...)                      │
│   .body_text(...)                   │
│   .build()                          │
│       ↓                             │
│   Returns HttpResponse              │
└──────┬──────────────────────────────┘
       │
       ▼
┌─────────────────────────────────────┐
│ Connection::send_response()         │
│ - Convert to_bytes()                │
│ - Write to TcpStream                │
│ - Handle chunked encoding           │
│ - Close connection                  │
└──────┬──────────────────────────────┘
       │
       ▼
  ┌──────────────┐
  │ HTTP Response │
  │ (sent to     │
  │  client)     │
  └──────────────┘
```

## Feature Matrix

```
┌─────────────────────┬──────┬──────┬──────┬──────┬──────┬──────┐
│ Feature             │ GET  │ HEAD │ POST │ PUT  │ DEL  │ PATCH│
├─────────────────────┼──────┼──────┼──────┼──────┼──────┼──────┤
│ Status Code         │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │
│ Headers             │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │
│ JSON Body           │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │
│ HTML Body           │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │
│ Static Files        │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │
│ Cookies             │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │
│ Chunked Encoding    │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │
│ Cache Headers       │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │
│ MIME Detection      │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │  ✅  │
└─────────────────────┴──────┴──────┴──────┴──────┴──────┴──────┘
```

## Cookie Handling Flow

```
┌─────────────────────────────────────────────────────────────────┐
│ ResponseBuilder::cookie_with_options(...)                      │
│                                                                  │
│ Input:  name: "user_session"                                  │
│         value: "session_12345"                                │
│         max_age: Some(3600)                                  │
│         path: "/"                                             │
│         http_only: true                                       │
│                                                                  │
│ Processing:                                                      │
│ 1. Format: "user_session=session_12345"                       │
│ 2. Add Max-Age: "; Max-Age=3600"                              │
│ 3. Add Path: "; Path=/"                                       │
│ 4. Add HttpOnly: "; HttpOnly"                                │
│                                                                  │
│ Output: "user_session=session_12345; Max-Age=3600; Path=/; │
│         HttpOnly"                                               │
│                                                                  │
│ Result: Stored in HttpResponse headers as Set-Cookie          │
└─────────────────────────────────────────────────────────────────┘
```

## MIME Type Detection Tree

```
file path
    ├─ .html → text/html
    ├─ .css → text/css
    ├─ .js → application/javascript
    ├─ .json → application/json
    ├─ .txt → text/plain
    ├─ .xml → application/xml
    ├─ image/
    │   ├─ .png → image/png
    │   ├─ .jpg/.jpeg → image/jpeg
    │   ├─ .gif → image/gif
    │   └─ .svg → image/svg+xml
    ├─ font/
    │   ├─ .woff → font/woff
    │   └─ .woff2 → font/woff2
    ├─ .pdf → application/pdf
    └─ [default] → application/octet-stream
```

## Chunked Encoding Process

```
Input Response:
    Status: 200 OK
    Body: "Hello World" (11 bytes)
    Chunked: true

Encoding Process:
    1. Convert body size to hex: 11 → B
    2. Format: "B\r\nHello World\r\n0\r\n"
    
Output Wire Format:
    B\r\n
    Hello World\r\n
    0\r\n
    
Client Receives:
    200 OK
    Transfer-Encoding: chunked
    [body chunks as defined above]
```

## Error Handling Path

```
ResponseBuilder::file("static/example.html")
         │
         ├─ File exists?
         │  │
         │  ├─ Yes → Ok(ResponseBuilder)
         │  │        └─ Continue chaining
         │  │            .header("Cache-Control", "...")
         │  │            .build()
         │  │
         │  └─ No → Err(io::Error)
         │          └─ Catch error
         │              └─ ResponseBuilder::new()
         │                  .status(404, "Not Found")
         │                  .body_text(ErrorPages::not_found())
         │                  .build()
         │
         └─ Return HttpResponse
```

## Concurrent Connection Handling

```
epoll_wait() detects ready connections
         │
         ├─ Listener (new connection)
         │  └─ accept_connection()
         │     └─ Register with epoll
         │
         ├─ Client Socket (data ready)
         │  └─ handle_client_data()
         │     └─ Parse HttpRequest
         │        └─ Router::handle(request)
         │           └─ Call handler
         │              └─ ResponseBuilder
         │                 └─ HttpResponse
         │                    └─ send_response()
         │                       └─ Write to socket
         │
         └─ Continue epoll_wait() for more events
```

## Performance Characteristics

```
Operation                   Complexity    Notes
─────────────────────────────────────────────────────
new()                       O(1)          Constant time
status()                    O(1)          Field assignment
header()                    O(1)          HashMap insert
body_text()                 O(n)          Copy string bytes
body_bytes()                O(1)          Move ownership
cookie()                    O(1)          Vec push
cookie_with_options()       O(k)          k = cookie string length
chunked()                   O(1)          Field assignment
file()                      O(n)          n = file size (disk I/O)
get_content_type()          O(m)          m = extension length
build()                     O(h)          h = header count

Total for typical response:  O(n)          Dominated by file read
                                          or body size
```

## Security Features

```
ResponseBuilder Security Capabilities:
    ├─ HttpOnly Cookies
    │  └─ Prevents JavaScript XSS access
    │
    ├─ Path Restriction
    │  └─ Limits cookie scope to domain path
    │
    ├─ Max-Age Expiration
    │  └─ Automatic session timeout
    │
    ├─ Content-Type Setting
    │  └─ Prevents MIME sniffing
    │
    ├─ Cache Control Headers
    │  └─ Prevents sensitive data caching
    │
    ├─ Custom Headers
    │  └─ Support for X-* security headers
    │
    ├─ File MIME Detection
    │  └─ Correct content type for static files
    │
    └─ Secure Flag Option (Ready)
       └─ Can add for HTTPS-only cookies
```

---

This architecture demonstrates a clean separation of concerns:
- **ResponseBuilder**: Configuration and construction
- **HttpResponse**: Serialization and transmission
- **Router**: Request routing
- **Handlers**: Business logic
- **epoll**: I/O multiplexing

The fluent API design makes code more readable while maintaining full flexibility.
