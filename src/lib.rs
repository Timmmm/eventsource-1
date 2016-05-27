#[macro_use] extern crate hyper;

mod error;

use error::Error;

use std::fmt;
use std::io::{BufRead, BufReader};
use std::time::Duration;
use hyper::client::{Client as HyperClient};
use hyper::client::response::Response;
use hyper::header::Headers;
use hyper::Url;

const DEFAULT_RETRY: u64 = 5000;

header! { (LastEventID, "Last-Event-ID") => [String] }

pub struct Client {
    hc: HyperClient,
    reader: Option<BufReader<Response>>,
    url: Url,
    last_event_id: Option<String>,
    retry: u64, // reconnection time in milliseconds
}

#[derive(Debug)]
pub struct Event {
    pub id: Option<String>,
    pub event_type: Option<String>,
    pub data: String,
}

enum ParseResult {
    Next,
    Dispatch,
}

impl Client {
    pub fn new(url: Url) -> Client {
        Client {
            hc: HyperClient::new(),
            reader: None,
            url: url,
            last_event_id: None,
            retry: DEFAULT_RETRY,
        }
    }

    fn next_request(&self) -> hyper::error::Result<Response> {
        let mut headers = Headers::new();
        if let Some(ref id) = self.last_event_id {
            headers.set(LastEventID(id.clone()));
        }
        self.hc.get(self.url.clone()).headers(headers).send()
    }

    fn parse_event_line(&mut self, line: &str, event: &mut Event) -> ParseResult {
        let line = if line.ends_with('\n') { &line[0..line.len()-1] } else { line };
        if line == "" {
            ParseResult::Dispatch
        } else {
            let (field, value) = if let Some(pos) = line.find(':') {
                let (f, v) = line.split_at(pos);
                // Strip : and an optional space.
                let v = &v[1..];
                let v = if v.starts_with(' ') { &v[1..] } else { v };
                (f, v)
            } else {
                (line, "")
            };
            
            match field {
                "event" => { event.event_type = Some(value.to_string()); },
                "data" => { event.data.push_str(value); event.data.push('\n'); },
                "id" => { event.id = Some(value.to_string()); self.last_event_id = Some(value.to_string()); }
                "retry" => {
                    if let Ok(retry) = value.parse::<u64>() {
                        self.retry = retry;
                    }
                },
                _ => () // ignored
            }

            ParseResult::Next
        }
    }
}

// Helper macro for Option<Result<...>>
macro_rules! try_option {
    ($e:expr) => (match $e {
        Ok(val) => val,
        Err(err) => return Some(Err(::std::convert::From::from(err))),
    });
}

// Iterate over the client to get events.
impl Iterator for Client {
    type Item = Result<Event, Error>;

    fn next(&mut self) -> Option<Result<Event, Error>> {
        if self.reader.is_none() {
            let req = try_option!(self.next_request());
            // We can only work with successful requests.
            // TODO: Should honor the `retry` timeout for the next iteration.
            if !req.status.is_success() {
                return Some(Err(Error::Http(req.status)));
            }
            let r = BufReader::new(req);
            self.reader = Some(r);
        }
        let mut event = Event::new();
        let mut line = String::new();

        // We can't have a mutable reference to the reader because of the &mut self call below.
        // The first unwrap() is safe as we're checking that above.
        while try_option!(self.reader.as_mut().unwrap().read_line(&mut line)) > 0 {
            match self.parse_event_line(&line, &mut event) {
                ParseResult::Dispatch => return Some(Ok(event)),
                ParseResult::Next => (),
            }
            line.clear();
        }
        // EOF, retry after timeout
        self.reader = None;
        std::thread::sleep(Duration::from_millis(self.retry));
        self.next()
    }
}

impl Event {
    fn new() -> Event {
        Event {
            id: None,
            event_type: None,
            data: "".to_string(),
        }
    }
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(ref id) = self.id {
            try!(write!(f, "id: {}\n", id));
        }
        if let Some(ref event_type) = self.event_type {
            try!(write!(f, "event: {}\n", event_type));
        }
        for line in self.data.lines() {
            try!(write!(f, "data: {}\n", line));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_event_display() {
        assert_eq!(
            "data: hello world\n",
            Event { id: None, event_type: None, data: "hello world".to_string() }.to_string());
        assert_eq!(
            "id: foo\ndata: hello world\n",
            Event { id: Some("foo".to_string()), event_type: None, data: "hello world".to_string() }.to_string());
        assert_eq!(
            "event: bar\ndata: hello world\n",
            Event { id: None, event_type: Some("bar".to_string()), data: "hello world".to_string() }.to_string());
    }

    #[test]
    fn multiline_event_display() {
        assert_eq!(
            "data: hello\ndata: world\n",
            Event { id: None, event_type: None, data: "hello\nworld".to_string() }.to_string());
        assert_eq!(
            "data: hello\ndata: \ndata: world\n",
            Event { id: None, event_type: None, data: "hello\n\nworld".to_string() }.to_string());
    }
}