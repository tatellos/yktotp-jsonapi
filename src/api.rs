use std::io;
use std::io::Read;
use std::io::Write;

use serde::{Deserialize, Serialize};

use crate::oath;
use crate::time;
use crate::yubikey;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum Request {
    AccountList,
    Code { account: String },
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Response {
    Code { account: String, code: String },
    AccountList { accounts: Vec<String> },
    Error { error: String },
}

#[derive(Debug)]
pub enum Error {
    Read,
    Write,
    Yubikey(yubikey::Error),
    Oath(oath::Error),
}

pub fn handle_request(request: &Request) -> Response {
    match request {
        Request::Code { account } => read_otp(&account),
        Request::AccountList => read_accounts_list(),
    }
}

fn read_accounts_list() -> Response {
    let accounts = yubikey::Yubikey::initialize()
        .map_err(Error::Yubikey)
        .and_then(|y| oath::list_credentials(&y).map_err(Error::Oath));

    match accounts {
        Ok(account_vec) => Response::AccountList {
            accounts: account_vec,
        },
        Err(e) => Response::Error {
            error: format!("{:?}", e),
        },
    }
}

fn read_otp(search_term: &str) -> Response {
    let timestamp = time::get_time();
    let code = yubikey::Yubikey::initialize()
        .map_err(Error::Yubikey)
        .and_then(|y| oath::calculate_fuzzy(&y, search_term, timestamp).map_err(Error::Oath));

    match code {
        Ok(code) => Response::Code {
            account: search_term.to_owned(),
            code: format!("{:06}", code),
        },
        Err(e) => Response::Error {
            error: format!("{:?}", e),
        },
    }
}

pub fn serve() -> Result<(), Error> {
    read().map(|r| handle_request(&r)).and_then(|r| write(&r))
}

fn read() -> Result<Request, Error> {
    read_input(&mut io::stdin()).and_then(|r| deserialize_request(&r))
}

fn write(response: &Response) -> Result<(), Error> {
    serialize_response(response).and_then(|r| write_output(&mut io::stdout(), &r))
}

fn read_input(buffer: &mut impl Read) -> Result<Vec<u8>, Error> {
    let mut raw_input_length: [u8; 4] = [0; 4];
    buffer
        .read_exact(&mut raw_input_length)
        .map_err(|_| Error::Read)?;
    let input_length =
        usize::try_from(u32::from_ne_bytes(raw_input_length)).map_err(|_| Error::Read)?;

    let mut raw_input = vec![0; input_length];
    buffer.read_exact(&mut raw_input).map_err(|_| Error::Read)?;

    Ok(raw_input)
}

fn write_output(buffer: &mut impl Write, raw_output: &[u8]) -> Result<(), Error> {
    buffer.write_all(raw_output).map_err(|_| Error::Write)
}

fn deserialize_request(raw_input: &[u8]) -> Result<Request, Error> {
    let input = std::str::from_utf8(raw_input).map_err(|_| Error::Read)?;
    serde_json::from_str(input).map_err(|_| Error::Read)
}

fn serialize_response(response: &Response) -> Result<Vec<u8>, Error> {
    let serialized = serde_json::to_string(response).map_err(|_| Error::Write)?;
    let raw_output = serialized.as_bytes();

    let output_length = u32::try_from(raw_output.len()).map_err(|_| Error::Write)?;
    let raw_output_length = u32::to_ne_bytes(output_length);

    Ok([&raw_output_length, raw_output].concat())
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(b"{\"type\":\"Code\",\"account\":\"rust-lang.org\"}", Request::Code { account: String::from("rust-lang.org")}; "works with proper json")]
    #[test_case(b"{\"type\":\"Code\",\"account\":\"rust-lang.org\",\"extra\":\"extra_field\"}", Request::Code { account: String::from("rust-lang.org")}; "ignores additional fields")]
    #[test_case(b"{\"type\":\"AccountList\"}", Request::AccountList; "works with account list request")]
    fn deserialize_request_succeeds(bytes: &[u8], request: Request) {
        let deserialized = deserialize_request(bytes).unwrap();
        assert_eq!(
            request, deserialized,
            "asserting equality of deserialized and expected request"
        )
    }

    #[test_case(b"{\"account\":\"rust-lang.org}"; "fails on illegal syntax")]
    #[test_case(b"{\"account\":22}"; "fails on integer type")]
    #[test_case(b"{\"no_account\":22}"; "fails on wrong key")]
    #[test_case(b"{}"; "fails on empty json")]
    #[test_case(b""; "fails on empty string")]
    #[test_case(b"{\"account\":\"rust-lang.org\"}231412"; "fails on trailing chars")]
    #[test_case(b"2134{\"account\":\"rust-lang.org\"}"; "fails on leading chars")]
    fn deserialize_request_fails_on_illegal_json(bytes: &[u8]) {
        assert!(
            matches!(deserialize_request(bytes), Err(Error::Read)),
            "asserting request deserialization results in error"
        )
    }

    #[test_case(& Response::Code{account: String::from("rust-lang.org"), code: String::from("123456")}, b"\x2B\x00\x00\x00{\"account\":\"rust-lang.org\",\"code\":\"123456\"}"; "succeeds for response with code")]
    #[test_case(& Response::AccountList{accounts: vec ! [String::from("rust-lang.org"), String::from("zombo.com")]}, b"\x2A\x00\x00\x00{\"accounts\":[\"rust-lang.org\",\"zombo.com\"]}"; "succeeds for response with account list")]
    #[test_case(& Response::Error{error: String::from("some error")}, b"\x16\x00\x00\x00{\"error\":\"some error\"}"; "succeeds for response with error")]
    fn serialize_response_succeeds(response: &Response, bytes: &[u8]) {
        let serialized = serialize_response(response).unwrap();
        assert_eq!(
            bytes, serialized,
            "assert serialized response equals expected bytes"
        )
    }

    #[test_case(
    b"\x1B\x00\x00\x00{\"account\":\"rust-lang.org\"}",
    b"{\"account\":\"rust-lang.org\"}";
    "succeeds for correct input length"
    )]
    #[test_case(
    b"\x1B\x00\x00\x00{\"account\":\"rust-lang.org\"}herearesomemorebytes",
    b"{\"account\":\"rust-lang.org\"}";
    "succeeds for additional bytes after input"
    )]
    fn read_input_succeeds(input_bytes: &[u8], output_bytes: &[u8]) {
        let buffer = input_bytes.to_vec();
        let read_bytes = read_input(&mut buffer.as_slice()).unwrap();
        assert_eq!(
            output_bytes, read_bytes,
            "assert read bytes equal expected bytes"
        )
    }

    #[test_case(b"\x1B\x00\x00\x00{\"account\":\"rust.org\"}"; "fails for too short input")]
    fn read_input_fails(input_bytes: &[u8]) {
        let buffer = input_bytes.to_vec();
        assert!(
            matches!(read_input(&mut buffer.as_slice()), Err(Error::Read)),
            "assert reading input fails"
        )
    }
}
