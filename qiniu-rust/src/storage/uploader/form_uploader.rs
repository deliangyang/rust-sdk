use super::{
    super::upload_token::Result as UploadTokenParseResult, upload_response_callback, BucketUploader, UpType,
    UploadLogger, UploadLoggerRecordBuilder, UploadResult,
};
use crate::utils::crc32;
use mime::Mime;
use multipart::client::lazy::Multipart;
use qiniu_http::{Error as HTTPError, Result as HTTPResult, RetryKind};
use serde_json::Value;
use std::{
    borrow::Cow,
    convert::TryInto,
    io::{Read, Result as IOResult, Seek, SeekFrom},
};

pub(super) struct FormUploaderBuilder<'u> {
    bucket_uploader: &'u BucketUploader,
    multipart: Multipart<'u, 'u>,
    on_uploading_progress: Option<&'u dyn Fn(usize, Option<usize>)>,
    upload_logger: Option<UploadLogger>,
}

pub(super) struct FormUploader<'u> {
    bucket_uploader: &'u BucketUploader,
    content_type: String,
    body: Vec<u8>,
    on_uploading_progress: Option<&'u dyn Fn(usize, Option<usize>)>,
    upload_logger: Option<UploadLogger>,
}

impl<'u> FormUploaderBuilder<'u> {
    pub(super) fn new(
        bucket_uploader: &'u BucketUploader,
        upload_token: &'u str,
    ) -> UploadTokenParseResult<FormUploaderBuilder<'u>> {
        let mut uploader = FormUploaderBuilder {
            bucket_uploader,
            multipart: Multipart::new(),
            on_uploading_progress: None,
            upload_logger: bucket_uploader
                .upload_logger_builder()
                .map(|builder| builder.upload_token(upload_token.into())),
        };
        uploader.multipart.add_text("token", upload_token);
        Ok(uploader)
    }

    pub(super) fn key(mut self, key: Cow<'u, str>) -> FormUploaderBuilder<'u> {
        self.multipart.add_text("key", key);
        self
    }

    pub(super) fn var(mut self, key: &str, value: Cow<'u, str>) -> FormUploaderBuilder<'u> {
        self.multipart.add_text("x:".to_owned() + key, value);
        self
    }

    pub(super) fn metadata(mut self, key: &str, value: Cow<'u, str>) -> FormUploaderBuilder<'u> {
        self.multipart.add_text("x-qn-meta-".to_owned() + key, value);
        self
    }

    pub(super) fn on_uploading_progress(
        mut self,
        callback: &'u dyn Fn(usize, Option<usize>),
    ) -> FormUploaderBuilder<'u> {
        self.on_uploading_progress = Some(callback);
        self
    }

    pub(super) fn seekable_stream<'n: 'u, R: Read + Seek + 'u>(
        mut self,
        mut stream: R,
        file_name: Option<Cow<'n, str>>,
        mime: Option<Mime>,
        checksum_enabled: bool,
    ) -> IOResult<FormUploader<'u>> {
        let mut crc32: Option<u32> = None;
        if checksum_enabled {
            crc32 = Some(crc32::from(&mut stream)?);
            stream.seek(SeekFrom::Start(0))?;
        }
        self.multipart.add_stream("file", stream, file_name, mime);
        if let Some(crc32) = crc32 {
            self.multipart.add_text("crc32", crc32.to_string());
        }
        self.upload_multipart()
    }

    pub(super) fn stream<'n: 'u, R: Read + 'u>(
        mut self,
        stream: R,
        mime: Option<Mime>,
        file_name: Option<Cow<'n, str>>,
        crc32: Option<u32>,
    ) -> IOResult<FormUploader<'u>> {
        self.multipart.add_stream("file", stream, file_name, mime);
        if let Some(crc32) = crc32 {
            self.multipart.add_text("crc32", crc32.to_string());
        }
        self.upload_multipart()
    }

    fn upload_multipart(mut self) -> IOResult<FormUploader<'u>> {
        let mut fields = self.multipart.prepare().map_err(|err| err.error)?;
        let mut body = Vec::with_capacity(
            self.bucket_uploader
                .http_client()
                .config()
                .upload_threshold()
                .try_into()
                .unwrap_or(1 << 22),
        );
        fields.read_to_end(&mut body)?;
        Ok(FormUploader {
            bucket_uploader: self.bucket_uploader,
            content_type: "multipart/form-data; boundary=".to_owned() + fields.boundary(),
            body,
            on_uploading_progress: self.on_uploading_progress,
            upload_logger: self.upload_logger,
        })
    }
}

impl<'u> FormUploader<'u> {
    pub(super) fn send(&self) -> HTTPResult<UploadResult> {
        let mut prev_err: Option<HTTPError> = None;
        for up_urls in self.bucket_uploader.up_urls_list().iter() {
            match self.send_form_request(&up_urls.iter().map(|url| url.as_ref()).collect::<Box<[&str]>>()) {
                Ok(value) => {
                    return Ok(value);
                }
                Err(err) => match err.retry_kind() {
                    RetryKind::RetryableError | RetryKind::HostUnretryableError | RetryKind::ZoneUnretryableError => {
                        prev_err = Some(err);
                    }
                    _ => {
                        return Err(err);
                    }
                },
            }
        }

        Err(prev_err.expect("FormUploader::send() should try at lease once, but not"))
    }

    fn send_form_request(&self, up_urls: &[&str]) -> HTTPResult<UploadResult> {
        let value: Value = self
            .bucket_uploader
            .http_client()
            .post("/", up_urls)
            .idempotent()
            .on_uploading_progress(&|uploaded, total| {
                if let Some(on_uploading_progress) = &self.on_uploading_progress {
                    (on_uploading_progress)(uploaded, Some(total));
                }
            })
            .on_response(&|response, duration| {
                let result = upload_response_callback(response);
                if result.is_ok() {
                    if let Some(upload_logger) = &self.upload_logger {
                        upload_logger.log(
                            UploadLoggerRecordBuilder::default()
                                .response(response)
                                .duration(duration)
                                .up_type(UpType::Form)
                                .sent(self.body.len())
                                .total_size(self.body.len())
                                .build()
                                .unwrap(),
                        );
                    }
                }
                result
            })
            .on_error(&|base_url, err, duration| {
                if let Some(upload_logger) = &self.upload_logger {
                    upload_logger.log({
                        let mut builder = UploadLoggerRecordBuilder::default()
                            .duration(duration)
                            .up_type(UpType::Form)
                            .http_error(err)
                            .total_size(self.body.len());
                        if let Some(base_url) = base_url {
                            builder = builder.host(base_url);
                        }
                        builder.build().unwrap()
                    });
                }
            })
            .accept_json()
            .raw_body(self.content_type.to_owned(), self.body.as_slice())
            .send()?
            .parse_json()?;
        Ok(value.into())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        super::{upload_policy::UploadPolicyBuilder, upload_token::UploadToken},
        BucketUploaderBuilder,
    };
    use crate::{config::ConfigBuilder, credential::Credential, http::DomainsManagerBuilder};
    use qiniu_http::Headers;
    use qiniu_test_utils::{
        http_call_mock::{CounterCallMock, ErrorResponseMock, JSONCallMock},
        temp_file::create_temp_file,
    };
    use serde_json::json;
    use std::{borrow::Cow, boxed::Box, error::Error, result::Result};

    #[test]
    fn test_storage_uploader_form_uploader_upload_file() -> Result<(), Box<dyn Error>> {
        let temp_path = create_temp_file(1 << 10)?.into_temp_path();
        let mock = CounterCallMock::new(JSONCallMock::new(
            200,
            Headers::new(),
            json!({"key": "abc", "hash": "def"}),
        ));
        let config = ConfigBuilder::default()
            .http_request_call(mock.as_boxed())
            .domains_manager(DomainsManagerBuilder::default().disable_url_resolution().build())
            .build()?;
        let policy = UploadPolicyBuilder::new_policy_for_bucket("test-bucket", &config).build();
        let result = BucketUploaderBuilder::new(
            "test-bucket".into(),
            vec![
                vec![Box::from("http://z1h1.com"), Box::from("http://z1h2.com")].into(),
                vec![Box::from("http://z2h1.com"), Box::from("http://z2h2.com")].into(),
            ]
            .into(),
            config,
            None,
        )?
        .build()
        .upload_token(UploadToken::from_policy(policy, get_credential()))
        .key("test:file")
        .upload_file(&temp_path, Some("file"), None)?;
        assert_eq!(result.key(), Some("abc"));
        assert_eq!(result.hash(), Some("def"));
        assert_eq!(mock.call_called(), 1);
        Ok(())
    }

    #[test]
    fn test_storage_uploader_form_uploader_upload_file_with_500_error() -> Result<(), Box<dyn Error>> {
        let temp_path = create_temp_file(1 << 10)?.into_temp_path();
        let mock = CounterCallMock::new(ErrorResponseMock::new(500, "test error"));
        let config = ConfigBuilder::default()
            .http_request_retries(3)
            .http_request_call(mock.as_boxed())
            .domains_manager(DomainsManagerBuilder::default().disable_url_resolution().build())
            .build()?;
        let policy = UploadPolicyBuilder::new_policy_for_bucket("test-bucket", &config).build();
        BucketUploaderBuilder::new(
            "test-bucket".into(),
            vec![
                vec![Box::from("http://z1h1.com"), Box::from("http://z1h2.com")].into(),
                vec![Box::from("http://z2h1.com"), Box::from("http://z2h2.com")].into(),
            ]
            .into(),
            config,
            None,
        )?
        .build()
        .upload_token(UploadToken::from_policy(policy, get_credential()))
        .key("test:file")
        .upload_file(&temp_path, Some("file"), None)
        .unwrap_err();
        assert_eq!(mock.call_called(), 16);
        Ok(())
    }

    #[test]
    fn test_storage_uploader_form_uploader_upload_file_with_503_error() -> Result<(), Box<dyn Error>> {
        let temp_path = create_temp_file(1 << 10)?.into_temp_path();
        let mock = CounterCallMock::new(ErrorResponseMock::new(503, "test error"));
        let config = ConfigBuilder::default()
            .http_request_retries(3)
            .http_request_call(mock.as_boxed())
            .domains_manager(DomainsManagerBuilder::default().disable_url_resolution().build())
            .build()?;
        let policy = UploadPolicyBuilder::new_policy_for_bucket("test-bucket", &config).build();
        BucketUploaderBuilder::new(
            "test-bucket".into(),
            vec![
                vec![Box::from("http://z1h1.com"), Box::from("http://z1h2.com")].into(),
                vec![Box::from("http://z2h1.com"), Box::from("http://z2h2.com")].into(),
            ]
            .into(),
            config,
            None,
        )?
        .build()
        .upload_token(UploadToken::from_policy(policy, get_credential()))
        .key("test:file")
        .upload_file(&temp_path, Some("file"), None)
        .unwrap_err();
        assert_eq!(mock.call_called(), 4);
        Ok(())
    }

    #[test]
    fn test_storage_uploader_form_uploader_upload_stream_with_500_error() -> Result<(), Box<dyn Error>> {
        let file = create_temp_file(1 << 10)?.into_file();
        let mock = CounterCallMock::new(ErrorResponseMock::new(500, "test error"));
        let config = ConfigBuilder::default()
            .http_request_retries(3)
            .http_request_call(mock.as_boxed())
            .domains_manager(DomainsManagerBuilder::default().disable_url_resolution().build())
            .build()?;
        let policy = UploadPolicyBuilder::new_policy_for_bucket("test-bucket", &config).build();
        BucketUploaderBuilder::new(
            "test-bucket".into(),
            vec![
                vec![Box::from("http://z1h1.com"), Box::from("http://z1h2.com")].into(),
                vec![Box::from("http://z2h1.com"), Box::from("http://z2h2.com")].into(),
            ]
            .into(),
            config,
            None,
        )?
        .build()
        .upload_token(UploadToken::from_policy(policy, get_credential()))
        .key("test:file")
        .never_be_resumeable()
        .upload_stream(&file, Some("file"), None)
        .unwrap_err();
        assert_eq!(mock.call_called(), 16);
        Ok(())
    }

    #[test]
    fn test_storage_uploader_form_uploader_upload_stream_with_503_error() -> Result<(), Box<dyn Error>> {
        let file = create_temp_file(1 << 10)?.into_file();
        let mock = CounterCallMock::new(ErrorResponseMock::new(503, "test error"));
        let config = ConfigBuilder::default()
            .http_request_retries(3)
            .http_request_call(mock.as_boxed())
            .domains_manager(DomainsManagerBuilder::default().disable_url_resolution().build())
            .build()?;
        let policy = UploadPolicyBuilder::new_policy_for_bucket("test-bucket", &config).build();
        BucketUploaderBuilder::new(
            "test-bucket".into(),
            vec![
                vec![Box::from("http://z1h1.com"), Box::from("http://z1h2.com")].into(),
                vec![Box::from("http://z2h1.com"), Box::from("http://z2h2.com")].into(),
            ]
            .into(),
            config,
            None,
        )?
        .build()
        .upload_token(UploadToken::from_policy(policy, get_credential()))
        .key("test:file")
        .never_be_resumeable()
        .upload_stream(&file, Some("file"), None)
        .unwrap_err();
        assert_eq!(mock.call_called(), 4);
        Ok(())
    }

    #[test]
    fn test_storage_uploader_form_uploader_upload_stream_with_400_incorrect_zone_error() -> Result<(), Box<dyn Error>> {
        let file = create_temp_file(1 << 10)?.into_file();
        let mock = CounterCallMock::new(ErrorResponseMock::new(400, "incorrect region, please use z3h1.com"));
        let config = ConfigBuilder::default()
            .http_request_retries(3)
            .http_request_call(mock.as_boxed())
            .domains_manager(DomainsManagerBuilder::default().disable_url_resolution().build())
            .build()?;
        let policy = UploadPolicyBuilder::new_policy_for_bucket("test-bucket", &config).build();
        BucketUploaderBuilder::new(
            "test-bucket".into(),
            vec![
                vec![Box::from("http://z1h1.com"), Box::from("http://z1h2.com")].into(),
                vec![Box::from("http://z2h1.com"), Box::from("http://z2h2.com")].into(),
            ]
            .into(),
            config,
            None,
        )?
        .build()
        .upload_token(UploadToken::from_policy(policy, get_credential()))
        .key("test:file")
        .never_be_resumeable()
        .upload_stream(&file, Some("file"), None)
        .unwrap_err();
        assert_eq!(mock.call_called(), 2);
        Ok(())
    }

    fn get_credential() -> Credential {
        Credential::new("abcdefghklmnopq", "1234567890")
    }
}
