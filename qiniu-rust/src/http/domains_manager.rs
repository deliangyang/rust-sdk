use super::super::storage::region::Region;
use chashmap::CHashMap;
use getset::{CopyGetters, Getters};
use rand::{rngs::ThreadRng, seq::SliceRandom};
use serde::{Deserialize, Serialize};
use std::{
    boxed::Box,
    env,
    fs::{File, OpenOptions},
    mem,
    net::{SocketAddr, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime},
};
use url::Url;

#[derive(Debug, Clone)]
struct Resolutions {
    socket_addrs: Box<[SocketAddr]>,
    cache_deadline: SystemTime,
}

#[derive(Debug, Clone, Getters, CopyGetters)]
pub struct DomainsManagerValue {
    frozen_urls: CHashMap<Box<str>, SystemTime>,
    resolutions: CHashMap<Box<str>, Resolutions>,

    #[get_copy = "pub"]
    frozen_urls_duration: Duration,

    #[get_copy = "pub"]
    resolutions_cache_lifetime: Duration,

    #[get_copy = "pub"]
    disable_url_resolution: bool,

    #[get_copy = "pub"]
    persistent_interval: Option<Duration>,
}

impl Default for DomainsManagerValue {
    fn default() -> Self {
        DomainsManagerValue {
            frozen_urls: CHashMap::new(),
            resolutions: CHashMap::new(),
            frozen_urls_duration: Duration::from_secs(10 * 60),
            resolutions_cache_lifetime: Duration::from_secs(60 * 60),
            disable_url_resolution: false,
            persistent_interval: Some(Duration::from_secs(30 * 60)),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PersistentDomainsManager {
    frozen_urls: Vec<PersistentFrozenURL>,
    resolutions: Vec<PersistentResolutions>,
    frozen_urls_duration: Duration,
    resolutions_cache_lifetime: Duration,
    disable_url_resolution: bool,
    persistent_interval: Option<Duration>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PersistentFrozenURL {
    url: Box<str>,
    frozen_until: SystemTime,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PersistentResolutions {
    url: Box<str>,
    socket_addrs: Box<[SocketAddr]>,
    cache_deadline: SystemTime,
}

impl DomainsManagerValue {
    fn load_from_file(path: &Path) -> persistent_error::Result<Self> {
        let persistent: PersistentDomainsManager = serde_json::from_reader(File::open(path)?)?;
        Ok(persistent.into())
    }

    fn save_to_file(&self, path: &Path) -> persistent_error::Result<()> {
        let persistent: PersistentDomainsManager = self.to_owned().into();
        serde_json::to_writer(
            OpenOptions::new().write(true).truncate(true).create(true).open(path)?,
            &persistent,
        )?;
        Ok(())
    }
}

impl From<PersistentDomainsManager> for DomainsManagerValue {
    fn from(persistent: PersistentDomainsManager) -> Self {
        let domains_manager = DomainsManagerValue {
            frozen_urls: CHashMap::new(),
            resolutions: CHashMap::new(),
            frozen_urls_duration: persistent.frozen_urls_duration,
            resolutions_cache_lifetime: persistent.resolutions_cache_lifetime,
            disable_url_resolution: persistent.disable_url_resolution,
            persistent_interval: persistent.persistent_interval,
        };

        for item in persistent.frozen_urls {
            domains_manager.frozen_urls.insert(item.url, item.frozen_until);
        }
        for item in persistent.resolutions {
            domains_manager.resolutions.insert(
                item.url,
                Resolutions {
                    socket_addrs: item.socket_addrs,
                    cache_deadline: item.cache_deadline,
                },
            );
        }

        domains_manager
    }
}

impl From<DomainsManagerValue> for PersistentDomainsManager {
    fn from(domains_manager: DomainsManagerValue) -> Self {
        let mut persistent = PersistentDomainsManager {
            frozen_urls: Vec::with_capacity(domains_manager.frozen_urls.len()),
            resolutions: Vec::with_capacity(domains_manager.resolutions.len()),
            frozen_urls_duration: domains_manager.frozen_urls_duration,
            resolutions_cache_lifetime: domains_manager.resolutions_cache_lifetime,
            disable_url_resolution: domains_manager.disable_url_resolution,
            persistent_interval: domains_manager.persistent_interval,
        };

        for (url, frozen_until) in domains_manager.frozen_urls {
            persistent.frozen_urls.push(PersistentFrozenURL {
                url: url,
                frozen_until: frozen_until,
            });
        }
        for (url, resolutions) in domains_manager.resolutions {
            persistent.resolutions.push(PersistentResolutions {
                url: url,
                socket_addrs: resolutions.socket_addrs,
                cache_deadline: resolutions.cache_deadline,
            });
        }

        persistent
    }
}

pub struct DomainsManagerBuilder {
    value: DomainsManagerValue,
    pre_resolve_urls: Vec<&'static str>,
    persistent_file_path: Option<PathBuf>,
}

impl DomainsManagerBuilder {
    pub fn frozen_urls_duration(mut self, frozen_urls_duration: Duration) -> Self {
        self.value.frozen_urls_duration = frozen_urls_duration;
        self
    }

    pub fn resolutions_cache_lifetime(mut self, resolutions_cache_lifetime: Duration) -> Self {
        self.value.resolutions_cache_lifetime = resolutions_cache_lifetime;
        self
    }

    pub fn disable_url_resolution(mut self) -> Self {
        self.value.disable_url_resolution = true;
        self
    }

    pub fn enable_url_resolution(mut self) -> Self {
        self.value.disable_url_resolution = false;
        self
    }

    pub fn auto_persistent_interval(mut self, persistent_interval: Duration) -> Self {
        self.value.persistent_interval = Some(persistent_interval);
        self
    }

    pub fn disable_auto_persistent(mut self) -> Self {
        self.value.persistent_interval = None;
        self
    }

    pub fn persistent<P: Into<PathBuf>>(mut self, persistent_file_path: Option<P>) -> Self {
        self.persistent_file_path = persistent_file_path.map(|path| path.into());
        self
    }

    pub fn pre_resolve_url(mut self, pre_resolve_url: &'static str) -> Self {
        self.pre_resolve_urls.push(pre_resolve_url);
        self
    }

    pub fn build(self) -> DomainsManager {
        let domains_manager = DomainsManager {
            inner: Arc::new(DomainsManagerInner {
                value: self.value,
                persistent_file_path: self.persistent_file_path,
                last_persistent_time: Mutex::new(Instant::now()),
            }),
        };
        if !self.pre_resolve_urls.is_empty() {
            Self::async_pre_resolve_urls(domains_manager.clone(), self.pre_resolve_urls);
        }
        domains_manager
    }

    fn async_pre_resolve_urls(domains_manager: DomainsManager, mut urls: Vec<&'static str>) {
        thread::spawn(move || {
            for _ in 0..3 {
                // TRY 3 times
                urls = urls
                    .into_iter()
                    .map(|url| (url, domains_manager.resolve(url)))
                    .filter_map(|(url, result)| result.err().map(|_| url))
                    .collect();
                if urls.is_empty() {
                    break;
                }
            }
        });
    }

    fn default_pre_resolve_urls() -> Vec<&'static str> {
        let mut urls = Vec::with_capacity(100);
        Region::all().iter().for_each(|region| {
            urls.extend_from_slice(&region.up_urls(false));
            urls.extend_from_slice(&region.up_urls(true));
            urls.extend_from_slice(&region.io_urls(false));
            urls.extend_from_slice(&region.io_urls(true));
            urls.push(region.rs_url(false));
            urls.push(region.rs_url(true));
            urls.push(region.rsf_url(false));
            urls.push(region.rsf_url(true));
            urls.push(region.api_url(false));
            urls.push(region.api_url(true));
        });
        urls.push(Region::uc_url(false));
        urls.push(Region::uc_url(true));
        urls
    }

    pub fn load_from_file<P: Into<PathBuf>>(
        persistent_file_path: P,
    ) -> persistent_error::Result<DomainsManagerBuilder> {
        let persistent_file_path = persistent_file_path.into();
        let value = DomainsManagerValue::load_from_file(&persistent_file_path)?;
        Ok(DomainsManagerBuilder {
            value: value,
            persistent_file_path: Some(persistent_file_path),
            pre_resolve_urls: vec![],
        })
    }

    pub fn create_new<P: Into<PathBuf>>(persistent_file_path: Option<P>) -> DomainsManagerBuilder {
        DomainsManagerBuilder {
            value: Default::default(),
            persistent_file_path: persistent_file_path.map(|path| path.into()),
            pre_resolve_urls: Self::default_pre_resolve_urls(),
        }
    }
}

impl Default for DomainsManagerBuilder {
    fn default() -> Self {
        let persistent_file_path = {
            let mut path = env::temp_dir();
            path.push("domains_manager.json");
            path
        };

        DomainsManagerValue::load_from_file(&persistent_file_path)
            .map(|value| DomainsManagerBuilder {
                value: value,
                persistent_file_path: Some(persistent_file_path.to_owned()),
                pre_resolve_urls: vec![],
            })
            .unwrap_or_else(|_| DomainsManagerBuilder {
                value: Default::default(),
                persistent_file_path: Some(persistent_file_path),
                pre_resolve_urls: Self::default_pre_resolve_urls(),
            })
    }
}

#[derive(Debug)]
struct DomainsManagerInner {
    value: DomainsManagerValue,
    persistent_file_path: Option<PathBuf>,
    last_persistent_time: Mutex<Instant>,
}

#[derive(Debug, Clone)]
pub struct DomainsManager {
    inner: Arc<DomainsManagerInner>,
}

impl DomainsManager {
    pub fn persistent(&self) -> Option<persistent_error::Result<()>> {
        let result = self.persistent_without_lock();
        match result {
            Some(Ok(_)) => {
                *self.inner.last_persistent_time.lock().unwrap() = Instant::now();
            }
            _ => {}
        }
        result
    }

    fn try_to_persistent_if_needed(&self) {
        if let Some(persistent_interval) = self.inner.value.persistent_interval {
            let mut last_persistent_time = self.inner.last_persistent_time.lock().unwrap();
            if last_persistent_time.elapsed() > persistent_interval {
                let _ = self.persistent_without_lock();
                *last_persistent_time = Instant::now();
            }
        }
    }

    fn persistent_without_lock(&self) -> Option<persistent_error::Result<()>> {
        if let Some(persistent_file_path) = &self.inner.persistent_file_path {
            return Some(self.inner.value.save_to_file(persistent_file_path));
        }
        None
    }

    pub fn choose<'a>(&self, urls: &'a [&'a str]) -> resolve_error::Result<Vec<Choice<'a>>> {
        let mut rng = rand::thread_rng();
        assert!(!urls.is_empty());
        let mut choices = Vec::<Choice>::with_capacity(urls.len());
        for url in urls.into_iter() {
            if !self.is_frozen_url(url)? {
                if let Some(choice) = self.make_choice(url, &mut rng) {
                    choices.push(choice);
                }
            }
        }
        if choices.is_empty() {
            choices.push(
                urls.into_iter()
                    .filter_map(|url| self.make_choice(url, &mut rng))
                    .min_by_key(|choice| {
                        self.inner
                            .value
                            .frozen_urls
                            .get(&Self::host_with_port(choice.url).unwrap())
                            .map(|time| time.duration_since(SystemTime::UNIX_EPOCH).unwrap())
                            .unwrap_or_else(|| Duration::from_secs(0))
                    })
                    .unwrap(),
            );
        }
        self.try_to_persistent_if_needed();
        Ok(choices)
    }

    pub fn freeze_url(&self, url: &str) -> url_parse_error::Result<()> {
        self.inner.value.frozen_urls.insert(
            Self::host_with_port(url)?,
            SystemTime::now() + self.inner.value.frozen_urls_duration,
        );
        self.try_to_persistent_if_needed();
        Ok(())
    }

    pub fn unfreeze_urls(&self) {
        self.inner.value.frozen_urls.clear();
        self.try_to_persistent_if_needed();
    }

    pub fn is_frozen_url(&self, url: &str) -> url_parse_error::Result<bool> {
        let url = Self::host_with_port(url)?;
        match self.inner.value.frozen_urls.get(&url) {
            Some(unfreeze_time) => {
                if *unfreeze_time < SystemTime::now() {
                    mem::drop(unfreeze_time);
                    self.inner.value.frozen_urls.remove(&url);
                    Ok(false)
                } else {
                    Ok(true)
                }
            }
            None => Ok(false),
        }
    }

    fn make_choice<'a>(&self, url: &'a str, rng: &mut ThreadRng) -> Option<Choice<'a>> {
        if self.inner.value.disable_url_resolution {
            return Some(Choice {
                url: url,
                socket_addrs: Vec::new().into(),
            });
        }
        self.resolve(url)
            .ok()
            .map(|mut results| {
                // TODO: Think about IP address speed testing
                results.shuffle(rng);
                results
            })
            .map(|results| Choice {
                url: url,
                socket_addrs: results,
            })
    }

    fn resolve(&self, url: &str) -> resolve_error::Result<Box<[SocketAddr]>> {
        let url = Self::host_with_port(url)?;
        match self.inner.value.resolutions.get(&url) {
            Some(resolution) => {
                if resolution.cache_deadline < SystemTime::now() {
                    mem::drop(resolution);
                    self.resolve_and_update_cache(&url)
                } else {
                    Ok(resolution.socket_addrs.clone())
                }
            }
            None => self.resolve_and_update_cache(&url),
        }
    }

    fn resolve_and_update_cache(&self, url: &str) -> resolve_error::Result<Box<[SocketAddr]>> {
        let mut result: Option<resolve_error::Result<Box<[SocketAddr]>>> = None;
        self.inner
            .value
            .resolutions
            .alter(url.into(), |resolutions| match resolutions {
                Some(resolutions) => {
                    if resolutions.cache_deadline < SystemTime::now() {
                        match self.make_resolutions(url) {
                            Ok(resolutions) => {
                                result = Some(Ok(resolutions.socket_addrs.clone()));
                                Some(resolutions)
                            }
                            Err(err) => {
                                result = Some(Err(err));
                                None
                            }
                        }
                    } else {
                        result = Some(Ok(resolutions.socket_addrs.clone()));
                        Some(resolutions)
                    }
                }
                None => match self.make_resolutions(url) {
                    Ok(resolutions) => {
                        result = Some(Ok(resolutions.socket_addrs.clone()));
                        Some(resolutions)
                    }
                    Err(err) => {
                        result = Some(Err(err));
                        None
                    }
                },
            });
        result.unwrap()
    }

    fn make_resolutions(&self, url: &str) -> resolve_error::Result<Resolutions> {
        Ok(Resolutions {
            socket_addrs: url.to_socket_addrs()?.collect::<Box<[_]>>().clone(),
            cache_deadline: SystemTime::now() + self.inner.value.resolutions_cache_lifetime,
        })
    }

    fn host_with_port(url: &str) -> url_parse_error::Result<Box<str>> {
        let parsed_url = Url::parse(&url)?;
        parsed_url
            .host_str()
            .map(|host| host.to_owned() + ":" + &parsed_url.port_or_known_default().unwrap().to_string())
            .map(|host_with_port| Ok(host_with_port.into()))
            .unwrap_or_else(|| Err(url_parse_error::ErrorKind::InvalidURL(url.into()).into()))
    }
}

impl Default for DomainsManager {
    fn default() -> Self {
        DomainsManagerBuilder::default().build()
    }
}

#[derive(Debug, Clone)]
pub struct Choice<'a> {
    pub url: &'a str,
    pub socket_addrs: Box<[SocketAddr]>,
}

pub mod url_parse_error {
    use error_chain::error_chain;
    use url::ParseError as URLParseError;

    error_chain! {
        errors {
            InvalidURL(url: String) {
                description("Invalid url")
                display("Invalid url: {}", url)
            }
        }

        foreign_links {
            URLParseError(URLParseError);
        }
    }
}

pub mod resolve_error {
    use super::url_parse_error;
    use error_chain::error_chain;
    use std::io::Error as IOError;

    error_chain! {
        links {
            URLParseError(url_parse_error::Error, url_parse_error::ErrorKind);
        }

        foreign_links {
            ResolveError(IOError);
        }
    }
}

pub mod persistent_error {
    use error_chain::error_chain;
    use serde_json::Error as JSONError;
    use std::io::Error as IOError;

    error_chain! {
        foreign_links {
            IOError(IOError);
            JSONError(JSONError);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qiniu_test_utils::temp_file;
    use std::{boxed::Box, error::Error, result::Result, thread};

    #[test]
    fn test_domains_manager_in_multiple_threads() -> Result<(), Box<dyn Error>> {
        let domains_manager = DomainsManagerBuilder::default()
            .frozen_urls_duration(Duration::from_secs(5))
            .build();
        assert!(!domains_manager.is_frozen_url("http://up.qiniup.com")?);

        let mut threads: Vec<thread::JoinHandle<()>> = Vec::with_capacity(10);
        {
            {
                let domains_manager = domains_manager.clone();
                threads.push(thread::Builder::new().name("thread0".into()).spawn(move || {
                    assert!(!domains_manager.is_frozen_url("http://up.qiniup.com").unwrap());

                    domains_manager.freeze_url("http://up.qiniup.com").unwrap();
                    assert!(domains_manager.is_frozen_url("http://up.qiniup.com").unwrap());

                    thread::sleep(Duration::from_secs(1));

                    domains_manager.freeze_url("http://upload.qiniup.com").unwrap();
                    assert!(domains_manager.is_frozen_url("http://upload.qiniup.com").unwrap());
                })?);
            }
            for thread_id in 1..=9 {
                let domains_manager = domains_manager.clone();
                threads.push(
                    thread::Builder::new()
                        .name(format!("thread{}", thread_id))
                        .spawn(move || {
                            assert!(!domains_manager.is_frozen_url("http://upload.qiniup.com").unwrap());
                            thread::sleep(Duration::from_secs(1));
                            assert!(domains_manager.is_frozen_url("http://up.qiniup.com").unwrap());
                            thread::sleep(Duration::from_secs(1));
                            assert!(domains_manager.is_frozen_url("http://up.qiniup.com").unwrap());
                            assert!(domains_manager.is_frozen_url("http://upload.qiniup.com/abc").unwrap());
                            assert!(!domains_manager.is_frozen_url("https://up.qiniup.com").unwrap());
                            assert!(!domains_manager.is_frozen_url("https://upload.qiniup.com/abc").unwrap());
                            thread::sleep(Duration::from_secs(1));
                            assert!(domains_manager.is_frozen_url("http://up.qiniup.com/").unwrap());
                            assert!(domains_manager.is_frozen_url("http://upload.qiniup.com").unwrap());
                            thread::sleep(Duration::from_millis(2500));
                            assert!(!domains_manager
                                .is_frozen_url("http://up.qiniup.com/def/fgh.xzy")
                                .unwrap());
                            assert!(!domains_manager.is_frozen_url("http://up.qiniup.com/").unwrap());
                            thread::sleep(Duration::from_secs(1));
                            assert!(!domains_manager.is_frozen_url("http://up.qiniup.com/").unwrap());
                            thread::sleep(Duration::from_secs(1));
                            assert!(!domains_manager
                                .is_frozen_url("http://upload.qiniup.com/def/fgh.xzy")
                                .unwrap());
                        })?,
                );
            }
        }
        threads.into_iter().for_each(|thread| thread.join().unwrap());
        Ok(())
    }

    #[test]
    fn test_domains_manager_choose() -> Result<(), Box<dyn Error>> {
        let domains_manager = DomainsManagerBuilder::default().build();
        domains_manager.freeze_url("http://up-z0.qiniup.com")?;
        domains_manager.freeze_url("http://up-z1.qiniup.com")?;

        let choices = domains_manager.choose(&["http://up-z0.qiniup.com", "http://up-z1.qiniup.com"])?;
        assert_eq!(choices.len(), 1);
        assert_eq!(choices.first().unwrap().url, "http://up-z0.qiniup.com");
        assert!(choices.first().unwrap().socket_addrs.len() > 5);

        let choices = domains_manager.choose(&[
            "http://up-z1.qiniup.com",
            "http://up-z2.qiniup.com",
            "http://unexisted-z3.qiniup.com",
            "http://unexisted-z4.qiniup.com",
        ])?;
        assert_eq!(choices.len(), 1);
        assert_eq!(choices.first().unwrap().url, "http://up-z2.qiniup.com");
        assert!(choices.first().unwrap().socket_addrs.len() > 0);
        Ok(())
    }

    #[test]
    fn test_domains_manager_persistent() -> Result<(), Box<dyn Error>> {
        let temp_path = temp_file::create_temp_file(0)?.into_temp_path();
        let temp_path: &Path = temp_path.as_ref();
        let domains_manager = DomainsManagerBuilder::create_new(Some(temp_path)).build();
        domains_manager.freeze_url("http://up-z0.qiniup.com")?;
        domains_manager.freeze_url("http://up-z1.qiniup.com")?;
        domains_manager.choose(&[
            "http://up-z1.qiniup.com",
            "http://up-z2.qiniup.com",
            "http://unexisted-z3.qiniup.com",
            "http://unexisted-z4.qiniup.com",
        ])?;
        match domains_manager.persistent() {
            Some(Ok(())) => {}
            _ => panic!(),
        }
        let inner = DomainsManagerValue::load_from_file(temp_path)?;
        assert!(inner.frozen_urls.contains_key("up-z0.qiniup.com:80".into()));
        assert!(inner.frozen_urls.contains_key("up-z1.qiniup.com:80".into()));
        assert!(inner.resolutions.contains_key("up-z2.qiniup.com:80".into()));
        assert!(!inner.resolutions.contains_key("unexisted-z3.qiniup.com:80".into()));
        assert!(!inner.resolutions.contains_key("unexisted-z4.qiniup.com:80".into()));

        let domains_manager = DomainsManagerBuilder::load_from_file(temp_path)?.build();
        assert!(domains_manager.is_frozen_url("http://up-z0.qiniup.com")?);
        assert!(domains_manager.is_frozen_url("http://up-z1.qiniup.com")?);
        Ok(())
    }

    #[test]
    fn test_domains_manager_auto_persistent() -> Result<(), Box<dyn Error>> {
        let temp_path = temp_file::create_temp_file(0)?.into_temp_path();
        let temp_path: &Path = temp_path.as_ref();
        let domains_manager = DomainsManagerBuilder::create_new(Some(temp_path))
            .auto_persistent_interval(Duration::from_secs(1))
            .build();
        domains_manager.freeze_url("http://up-z0.qiniup.com")?;
        DomainsManagerValue::load_from_file(temp_path).unwrap_err();
        thread::sleep(Duration::from_secs(1));
        domains_manager.freeze_url("http://up-z1.qiniup.com")?;
        DomainsManagerValue::load_from_file(temp_path)?;
        Ok(())
    }
}
