use std::net::{TcpListener, TcpStream};
use std::io::{self, Read, Write};
use std::time::Duration;

struct Server {
    listener: TcpListener,
    port: u16,
}

impl Server {
    pub fn new(port: u16) -> io::Result<Server> {
        // let address = format!("127.0.0.1:{}", port);
        let listener = TcpListener::bind(format!("127.0.0.1:{}", port))?;
        listener.set_nonblocking(true)?;
        
        println!("Server started on port http://localhost:{}/", port);
        
        Ok(Server {
            listener,
            port,
        })
    }
    
    pub fn run(&self) -> io::Result<()> {
        loop {
            self.accept_connections()?;
            // Add a small sleep to prevent CPU from maxing out
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    
    fn accept_connections(&self) -> io::Result<()> {
        match self.listener.accept() {
            Ok((stream, addr)) => {
                println!("New connection from: {}", addr);
                self.handle_client(stream)?;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // No connection available, just continue
                return Ok(());
            }
            Err(e) => {
                eprintln!("Error accepting connection: {}", e);
            }
        }
        Ok(())
    }
    
    fn handle_client(&self, mut stream: TcpStream) -> io::Result<()> {
        // Set read timeout
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        
        let mut buffer = [0; 4096];
        match stream.read(&mut buffer) {
            Ok(n) if n == 0 => {
                println!("Connection closed by client");
            }
            Ok(n) => {
                if let Ok(request) = String::from_utf8(buffer[..n].to_vec()) {
                    println!("Received request:\n{}", request);
                    self.send_response(&mut stream)?;
                }
            }
            Err(e) => {
                eprintln!("Error reading from client: {}", e);
            }
        }
        Ok(())
    }
    
    fn send_response(&self, stream: &mut TcpStream) -> io::Result<()> {
        let response = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: text/html\r\n\
             Content-Length: 98\r\n\
             \r\n\
             <html><body><h1>Hello from Rust Server on port {}!</h1><p>Your request was received.</p></body></html>",
            self.port
        );
        
        stream.write_all(response.as_bytes())?;
        stream.flush()?;
        Ok(())
    }
}

fn main() -> io::Result<()> {
    let server = Server::new(8080)?;
    server.run()
}