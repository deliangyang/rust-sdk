use super::{
    super::{token::Token, DomainsManager, Response},
    Parts, Request,
};
use crate::config::Config;
use error_chain::error_chain;
use qiniu_http::{Error as HTTPError, HeaderName, HeaderValue, Headers, Method, Result as HTTPResult};
use serde::Serialize;
use std::{borrow::Cow, collections::HashMap, result::Result as StdResult, time::Duration};

error_chain! {
    foreign_links {
        JSONError(serde_json::error::Error);
        FormError(serde_urlencoded::ser::Error);
    }
}

pub(crate) struct Builder<'a> {
    parts: Parts<'a>,
    domains_manager: DomainsManager,
}

impl<'a> Builder<'a> {
    pub(crate) fn new(config: Config, method: Method, path: &'a str, hosts: &'a [&'a str]) -> Builder<'a> {
        Builder {
            domains_manager: config.domains_manager().clone(),
            parts: Parts {
                config: config,
                method: method,
                hosts: hosts,
                path: path,
                query: None,
                headers: None,
                body: None,
                token: Token::None,
                read_body: false,
                idempotent: false,
                follow_redirection: false,
                on_uploading_progress: None,
                on_downloading_progress: None,
                on_retry_request: None,
                on_host_failed: None,
                on_response: None,
                on_failed: None,
            },
        }
    }

    pub(crate) fn header<K: Into<HeaderName<'a>>, V: Into<HeaderValue<'a>>>(mut self, key: K, value: V) -> Builder<'a> {
        match &mut self.parts.headers {
            Some(headers) => {
                headers.insert(key.into(), value.into());
            }
            None => {
                self.parts.headers = Some({
                    let mut h = Headers::with_capacity(4);
                    h.insert(key.into(), value.into());
                    h
                });
            }
        }
        self
    }

    pub(crate) fn query<K: Into<Cow<'a, str>>, V: Into<Cow<'a, str>>>(mut self, key: K, value: V) -> Builder<'a> {
        match self.parts.query {
            Some(ref mut query) => {
                query.insert(key.into(), value.into());
            }
            None => {
                self.parts.query = Some({
                    let mut q = HashMap::with_capacity(4);
                    q.insert(key.into(), value.into());
                    q
                });
            }
        }
        self
    }

    pub(crate) fn token(mut self, token: Token) -> Builder<'a> {
        self.parts.token = token;
        self
    }

    pub(crate) fn idempotent(mut self) -> Builder<'a> {
        self.parts.idempotent = true;
        self
    }

    pub(crate) fn follow_redirection(mut self) -> Builder<'a> {
        self.parts.follow_redirection = true;
        self
    }

    pub(crate) fn on_uploading_progress(mut self, callback: &'a dyn Fn(usize, usize)) -> Builder<'a> {
        self.parts.on_uploading_progress = Some(callback);
        self
    }

    pub(crate) fn on_downloading_progress(mut self, callback: &'a dyn Fn(usize, usize)) -> Builder<'a> {
        self.parts.on_downloading_progress = Some(callback);
        self
    }

    pub(crate) fn on_retry_request(
        mut self,
        callback: &'a dyn Fn(&str, &HTTPError, usize, usize, Duration),
    ) -> Builder<'a> {
        self.parts.on_retry_request = Some(callback);
        self
    }

    pub(crate) fn on_host_failed(mut self, callback: &'a dyn Fn(&str, &HTTPError, Duration)) -> Builder<'a> {
        self.parts.on_host_failed = Some(callback);
        self
    }

    pub(crate) fn on_response(
        mut self,
        callback: &'a dyn Fn(&mut Response, Duration) -> HTTPResult<()>,
    ) -> Builder<'a> {
        self.parts.on_response = Some(callback);
        self
    }

    pub(crate) fn on_failed(mut self, callback: &'a dyn Fn(&HTTPError, Duration)) -> Builder<'a> {
        self.parts.on_failed = Some(callback);
        self
    }

    pub(crate) fn accept_json(mut self) -> Builder<'a> {
        self = self.header("Accept", "application/json");
        self.parts.read_body = true;
        self
    }

    pub(crate) fn no_body(self) -> Request<'a> {
        self.build()
    }

    pub(crate) fn raw_body<T: Into<Vec<u8>>, S: Into<HeaderValue<'a>>>(
        mut self,
        content_type: S,
        body: T,
    ) -> Request<'a> {
        self = self.header("Content-Type", content_type);
        self.parts.body = Some(body.into());
        self.build()
    }

    pub(crate) fn json_body<T: Serialize>(mut self, body: &T) -> StdResult<Request<'a>, Error> {
        let serialized_body = serde_json::to_vec(body)?;
        self = self.header("Content-Type", "application/json");
        self.parts.body = Some(serialized_body);
        Ok(self.build())
    }

    pub(crate) fn form_body<T: Serialize>(mut self, body: &T) -> StdResult<Request<'a>, Error> {
        let serialized_body = serde_urlencoded::to_string(body)?;
        self = self.header("Content-Type", "application/x-www-form-urlencoded");
        self.parts.body = Some(serialized_body.into_bytes());
        Ok(self.build())
    }

    fn build(self) -> Request<'a> {
        Request {
            parts: self.parts,
            domains_manager: self.domains_manager,
        }
    }
}
