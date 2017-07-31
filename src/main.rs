#[macro_use]
extern crate nom;

mod parser;

use parser::*;

mod jobqueue;

use jobqueue::*;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::str;
use std::sync::{Arc, Mutex};
use std::thread;

struct Server {
    stream: TcpStream,
    job_queue: Arc<Mutex<JobQueue>>,
}

impl Server {
    fn new(stream: TcpStream, job_queue: Arc<Mutex<JobQueue>>) -> Server {
        Server {
            stream: stream,
            job_queue: job_queue,
        }
    }

    fn run(&mut self) {
        let mut parser = Parser::new();

        loop {
            if parser.is_incomplete() {
                parser.allocate();
                let len = {
                    let pos = parser.written;
                    let mut buffer = parser.get_mut();

                    // read socket
                    match self.stream.read(&mut buffer[pos..]) {
                        Ok(r) => r,
                        Err(err) => {
                            println!("Reading from client: {:?}", err);
                            break;
                        }
                    }
                };
                parser.written += len;

                // client closed connection
                if len == 0 {
                    println!("Client closed connection");
                    break;
                }
            }

            match parser.next() {
                Ok(command) => {
                    println!("Received command {:?}", command);

                    let mut job_queue = self.job_queue.lock().unwrap();

                    match command {
                        Command::Put {data} => {
                            let mut alloc_data = Vec::new();
                            alloc_data.extend_from_slice(data);

                            let id = job_queue.put(1, 1, 1, alloc_data);

                            let response = format!("INSERTED {}\r\n", id);

                            self.stream.write(response.as_bytes());
                        },
                        Command::Reserve => {
                            let (job_id, job_data) = job_queue.reserve();

                            let header = format!("RESERVED {} {}\r\n", job_id, job_data.len());

                            self.stream.write(header.as_bytes());
                            self.stream.write(job_data.as_slice());
                            self.stream.write(b"\r\n");
                        },
                        Command::Delete {id} => {
                            let id = str::from_utf8(id)
                                .unwrap()
                                .parse::<u8>()
                                .unwrap();

                            match job_queue.delete(&id) {
                                Some(_) => self.stream.write(b"DELETED\r\n"),
                                None => self.stream.write(b"NOT FOUND\r\n"),
                            };
                        },
                        Command::Release {id, pri, delay} => {
                            let id = str::from_utf8(id)
                                .unwrap()
                                .parse::<u8>()
                                .unwrap();

                            match job_queue.release(&id) {
                                Some(_) => self.stream.write(b"RELEASED\r\n"),
                                None => self.stream.write(b"NOT FOUND\r\n"),
                            };
                        },
                        Command::Watch {tube} => {
                            self.stream.write(b"WATCHING 1\r\n");
                        },
                        Command::ListTubes {} => {
                            let tube_list = "default";
                            self.stream.write(format!(
                                "OK {}\r\n{}\r\n",
                                tube_list.len(),
                                tube_list
                            ).as_bytes());
                        },
                        Command::StatsTube {tube} => {
                            let stats = "name: default
current-jobs-urgent: 0
current-jobs-ready: 0
current-jobs-reserved: 0
current-jobs-delayed: 0
current-jobs-buried: 0
total-jobs: 0
current-using: 0
current-waiting: 0
current-watching: 0
pause: 0
cmd-delete: 0
cmd-pause-tube: 0
pause-time-left: 0
";
                            self.stream.write(format!(
                                "OK {}\r\n{}\r\n",
                                stats.len(),
                                stats
                            ).as_bytes());
                        },
                        Command::UseTube {tube} => {
                            self.stream.write(format!("USING {:?}\r\n", tube).as_bytes());
                        },
                        Command::PeekReady {} => {
                            self.stream.write(b"NOT_FOUND\r\n");
                        },
                        Command::PeekDelayed {} => {
                            self.stream.write(b"NOT_FOUND\r\n");
                        },
                        Command::PeekBuried {} => {
                            self.stream.write(b"NOT_FOUND\r\n");
                        },
                    };
                },
                Err(err) => {
                    match err {
                        // if it's incomplete, keep adding to the buffer
                        ParseError::Incomplete => {
                            println!("Incomplete");
                            continue;
                        }
                        _ => {
                            println!("Protocol error from client: {:?}", err);
                            break;
                        }
                    }
                }
            };
        }
    }
}

fn main() {
    let listener = TcpListener::bind("127.0.0.1:11300").unwrap();

    let job_queue = Arc::new(Mutex::new(JobQueue::new()));

    for stream in listener.incoming() {
        match stream {
            Err(_) => panic!("error listen"),
            Ok(stream) => {
                let job_queue = job_queue.clone();
                thread::spawn(move || {
                    println!("client connected");

                    let mut server = Server::new(stream, job_queue);
                    server.run();
                });
            },
        };
    }
}
