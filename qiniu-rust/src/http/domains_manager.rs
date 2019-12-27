use crate::{config::Config, storage::region::Region, utils::global_thread_pool};
use assert_impl::assert_impl;
use chashmap::CHashMap;
use dirs::cache_dir;
use rand::{rngs::ThreadRng, seq::SliceRandom, thread_rng, Rng};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    boxed::Box,
    cell::RefCell,
    collections::HashSet,
    env::temp_dir,
    fs::{create_dir_all, File, OpenOptions},
    io::Error as IOError,
    mem::drop,
    net::{SocketAddr, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread::sleep,
    time::{Duration, Instant, SystemTime},
};
use tap::TapOps;
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone)]
struct CachedResolutions {
    socket_addrs: Box<[SocketAddr]>,
    cache_deadline: SystemTime,
}

#[derive(Debug, Clone)]
struct DomainsManagerInnerData {
    frozen_urls: CHashMap<Box<str>, SystemTime>,
    resolutions: CHashMap<Box<str>, CachedResolutions>,
    url_frozen_duration: Duration,
    resolutions_cache_lifetime: Duration,
    url_resolution_disabled: bool,
    persistent_interval: Option<Duration>,
    refresh_resolutions_interval: Option<Duration>,
    url_resolve_retries: usize,
    url_resolve_retry_delay: Duration,
}

impl Default for DomainsManagerInnerData {
    fn default() -> Self {
        DomainsManagerInnerData {
            frozen_urls: CHashMap::new(),
            resolutions: CHashMap::new(),
            url_frozen_duration: default::url_frozen_duration(),
            resolutions_cache_lifetime: default::resolutions_cache_lifetime(),
            url_resolution_disabled: default::url_resolution_disabled(),
            persistent_interval: default::persistent_interval(),
            refresh_resolutions_interval: default::refresh_resolutions_interval(),
            url_resolve_retries: default::url_resolve_retries(),
            url_resolve_retry_delay: default::url_resolve_retry_delay(),
        }
    }
}

mod default {
    use super::*;

    #[inline]
    pub const fn url_frozen_duration() -> Duration {
        Duration::from_secs(10 * 60)
    }

    #[inline]
    pub const fn resolutions_cache_lifetime() -> Duration {
        Duration::from_secs(60 * 60)
    }

    #[inline]
    pub const fn url_resolution_disabled() -> bool {
        false
    }

    #[inline]
    pub const fn persistent_interval() -> Option<Duration> {
        Some(Duration::from_secs(30 * 60))
    }

    #[inline]
    pub const fn refresh_resolutions_interval() -> Option<Duration> {
        Some(Duration::from_secs(30 * 60))
    }

    #[inline]
    pub const fn url_resolve_retries() -> usize {
        10
    }

    #[inline]
    pub const fn url_resolve_retry_delay() -> Duration {
        Duration::from_secs(1)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PersistentDomainsManager {
    frozen_urls: Vec<PersistentFrozenURL>,
    resolutions: Vec<PersistentResolutions>,
    url_frozen_duration: Duration,
    resolutions_cache_lifetime: Duration,
    url_resolution_disabled: bool,
    persistent_interval: Option<Duration>,
    refresh_resolutions_interval: Option<Duration>,
    url_resolve_retries: usize,
    url_resolve_retry_delay: Duration,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PersistentFrozenURL {
    base_url: Box<str>,
    frozen_until: SystemTime,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PersistentResolutions {
    base_url: Box<str>,
    socket_addrs: Box<[SocketAddr]>,
    cache_deadline: SystemTime,
}

impl DomainsManagerInnerData {
    fn load_from_file(path: &Path) -> PersistentResult<Self> {
        let persistent: PersistentDomainsManager = serde_json::from_reader(File::open(path)?)?;
        Ok(persistent.into())
    }

    fn save_to_file(&self, path: &Path) -> PersistentResult<()> {
        let persistent: PersistentDomainsManager = self.to_owned().into();
        serde_json::to_writer(
            OpenOptions::new().write(true).truncate(true).create(true).open(path)?,
            &persistent,
        )?;
        Ok(())
    }
}

impl From<PersistentDomainsManager> for DomainsManagerInnerData {
    fn from(persistent: PersistentDomainsManager) -> Self {
        let domains_manager = DomainsManagerInnerData {
            frozen_urls: CHashMap::new(),
            resolutions: CHashMap::new(),
            url_frozen_duration: persistent.url_frozen_duration,
            resolutions_cache_lifetime: persistent.resolutions_cache_lifetime,
            url_resolution_disabled: persistent.url_resolution_disabled,
            persistent_interval: persistent.persistent_interval,
            refresh_resolutions_interval: persistent.refresh_resolutions_interval,
            url_resolve_retries: persistent.url_resolve_retries,
            url_resolve_retry_delay: persistent.url_resolve_retry_delay,
        };

        for item in persistent.frozen_urls {
            domains_manager.frozen_urls.insert(item.base_url, item.frozen_until);
        }
        for item in persistent.resolutions {
            domains_manager.resolutions.insert(
                item.base_url,
                CachedResolutions {
                    socket_addrs: item.socket_addrs,
                    cache_deadline: item.cache_deadline,
                },
            );
        }

        domains_manager
    }
}

impl From<DomainsManagerInnerData> for PersistentDomainsManager {
    fn from(domains_manager: DomainsManagerInnerData) -> Self {
        let mut persistent = PersistentDomainsManager {
            frozen_urls: Vec::with_capacity(domains_manager.frozen_urls.len()),
            resolutions: Vec::with_capacity(domains_manager.resolutions.len()),
            url_frozen_duration: domains_manager.url_frozen_duration,
            resolutions_cache_lifetime: domains_manager.resolutions_cache_lifetime,
            url_resolution_disabled: domains_manager.url_resolution_disabled,
            persistent_interval: domains_manager.persistent_interval,
            refresh_resolutions_interval: domains_manager.refresh_resolutions_interval,
            url_resolve_retries: domains_manager.url_resolve_retries,
            url_resolve_retry_delay: domains_manager.url_resolve_retry_delay,
        };

        for (base_url, frozen_until) in domains_manager.frozen_urls {
            persistent
                .frozen_urls
                .push(PersistentFrozenURL { base_url, frozen_until });
        }
        for (base_url, resolutions) in domains_manager.resolutions {
            persistent.resolutions.push(PersistentResolutions {
                base_url,
                socket_addrs: resolutions.socket_addrs,
                cache_deadline: resolutions.cache_deadline,
            });
        }

        persistent
    }
}

pub struct DomainsManagerBuilder {
    inner_data: DomainsManagerInnerData,
    pre_resolution_urls: HashSet<Cow<'static, str>>,
    is_pre_resolution_async: bool,
    persistent_file_path: Option<PathBuf>,
}

impl DomainsManagerBuilder {
    pub fn url_frozen_duration(mut self, url_frozen_duration: Duration) -> Self {
        self.inner_data.url_frozen_duration = url_frozen_duration;
        self
    }

    pub fn resolutions_cache_lifetime(mut self, resolutions_cache_lifetime: Duration) -> Self {
        self.inner_data.resolutions_cache_lifetime = resolutions_cache_lifetime;
        self
    }

    pub fn disable_url_resolution(mut self) -> Self {
        self.inner_data.url_resolution_disabled = true;
        self
    }

    pub fn enable_url_resolution(mut self) -> Self {
        self.inner_data.url_resolution_disabled = false;
        self
    }

    pub fn auto_persistent_interval(mut self, persistent_interval: Duration) -> Self {
        self.inner_data.persistent_interval = Some(persistent_interval);
        self
    }

    pub fn disable_auto_persistent(mut self) -> Self {
        self.inner_data.persistent_interval = None;
        self
    }

    pub fn url_resolve_retries(mut self, url_resolve_retries: usize) -> Self {
        self.inner_data.url_resolve_retries = url_resolve_retries;
        self
    }

    pub fn url_resolve_retry_delay(mut self, url_resolve_retry_delay: Duration) -> Self {
        self.inner_data.url_resolve_retry_delay = url_resolve_retry_delay;
        self
    }

    pub fn persistent<P: Into<PathBuf>>(mut self, persistent_file_path: Option<P>) -> Self {
        self.persistent_file_path = persistent_file_path.map(|path| path.into());
        self
    }

    pub fn pre_resolve_url<U: Into<Cow<'static, str>>>(mut self, pre_resolve_url: U) -> Self {
        self.pre_resolution_urls.insert(pre_resolve_url.into());
        self
    }

    pub fn async_pre_resolve(mut self) -> Self {
        self.is_pre_resolution_async = true;
        self
    }

    pub fn sync_pre_resolve(mut self) -> Self {
        self.is_pre_resolution_async = false;
        self
    }

    pub fn build(self) -> DomainsManager {
        let domains_manager = DomainsManager {
            inner: Arc::new(DomainsManagerInner {
                inner_data: self.inner_data,
                persistent_file_path: self.persistent_file_path,
                last_persistent_time: Mutex::new(Instant::now()),
                last_refresh_time: Mutex::new(Instant::now()),
            }),
        };
        if !domains_manager.inner.inner_data.url_resolution_disabled {
            if !self.pre_resolution_urls.is_empty() {
                if self.is_pre_resolution_async {
                    domains_manager.async_resolve_urls(self.pre_resolution_urls);
                } else {
                    domains_manager.sync_resolve_urls(self.pre_resolution_urls);
                }
            } else {
                domains_manager.async_refresh_resolutions_without_update_refresh_time();
            }
        }
        domains_manager
    }

    fn default_pre_resolve_urls() -> HashSet<Cow<'static, str>> {
        let mut urls = HashSet::with_capacity(100);
        Region::all().iter().for_each(|region| {
            region.up_urls_ref(false).into_iter().for_each(|url| {
                urls.insert(Cow::Borrowed(url));
            });
            region.up_urls_ref(true).into_iter().for_each(|url| {
                urls.insert(Cow::Borrowed(url));
            });
            region.io_urls_ref(false).into_iter().for_each(|url| {
                urls.insert(Cow::Borrowed(url));
            });
            region.io_urls_ref(true).into_iter().for_each(|url| {
                urls.insert(Cow::Borrowed(url));
            });
            region.rs_urls_ref(false).into_iter().for_each(|url| {
                urls.insert(Cow::Borrowed(url));
            });
            region.rs_urls_ref(true).into_iter().for_each(|url| {
                urls.insert(Cow::Borrowed(url));
            });
            region.rsf_urls_ref(false).into_iter().for_each(|url| {
                urls.insert(Cow::Borrowed(url));
            });
            region.rsf_urls_ref(true).into_iter().for_each(|url| {
                urls.insert(Cow::Borrowed(url));
            });
            region.api_urls_ref(false).into_iter().for_each(|url| {
                urls.insert(Cow::Borrowed(url));
            });
            region.api_urls_ref(true).into_iter().for_each(|url| {
                urls.insert(Cow::Borrowed(url));
            });
        });
        urls
    }

    pub fn load_from_file<P: Into<PathBuf>>(persistent_file_path: P) -> PersistentResult<DomainsManagerBuilder> {
        let persistent_file_path = persistent_file_path.into();
        let inner_data = DomainsManagerInnerData::load_from_file(&persistent_file_path)?;
        Ok(DomainsManagerBuilder {
            inner_data,
            persistent_file_path: Some(persistent_file_path),
            pre_resolution_urls: Default::default(),
            is_pre_resolution_async: false,
        })
    }

    pub fn create_new<P: Into<PathBuf>>(persistent_file_path: Option<P>) -> DomainsManagerBuilder {
        DomainsManagerBuilder {
            inner_data: Default::default(),
            persistent_file_path: persistent_file_path.map(|path| path.into()),
            pre_resolution_urls: Self::default_pre_resolve_urls(),
            is_pre_resolution_async: true,
        }
    }
}

impl Default for DomainsManagerBuilder {
    fn default() -> Self {
        let persistent_file_path = {
            let mut default_path = cache_dir().unwrap_or_else(temp_dir);
            default_path.push("qiniu_sdk");
            default_path = create_dir_all(&default_path)
                .map(|_| default_path)
                .unwrap_or_else(|_| temp_dir());
            default_path.push("domains_manager.json");
            default_path
        };

        DomainsManagerInnerData::load_from_file(&persistent_file_path)
            .map(|inner_data| DomainsManagerBuilder {
                inner_data,
                persistent_file_path: Some(persistent_file_path.to_owned()),
                pre_resolution_urls: Default::default(),
                is_pre_resolution_async: false,
            })
            .unwrap_or_else(|_| DomainsManagerBuilder {
                inner_data: Default::default(),
                persistent_file_path: Some(persistent_file_path),
                pre_resolution_urls: Self::default_pre_resolve_urls(),
                is_pre_resolution_async: true,
            })
    }
}

#[derive(Debug)]
struct DomainsManagerInner {
    inner_data: DomainsManagerInnerData,
    persistent_file_path: Option<PathBuf>,
    last_persistent_time: Mutex<Instant>,
    last_refresh_time: Mutex<Instant>,
}

#[derive(Debug, Clone)]
pub struct DomainsManager {
    inner: Arc<DomainsManagerInner>,
}

impl DomainsManager {
    pub fn persistent(&self) -> Option<PersistentResult<()>> {
        let result = self.persistent_without_lock();
        if let Some(Ok(_)) = result {
            *self.inner.last_persistent_time.lock().unwrap() = Instant::now();
        }
        result
    }

    fn try_to_persistent_if_needed(&self) {
        if let Some(persistent_interval) = self.inner.inner_data.persistent_interval {
            let mut last_persistent_time = self.inner.last_persistent_time.lock().unwrap();
            if last_persistent_time.elapsed() > persistent_interval {
                let _ = self.persistent_without_lock();
                *last_persistent_time = Instant::now();
            }
        }
    }

    fn persistent_without_lock(&self) -> Option<PersistentResult<()>> {
        self.inner
            .persistent_file_path
            .as_ref()
            .map(|persistent_file_path| self.inner.inner_data.save_to_file(persistent_file_path))
    }

    pub fn choose<'a>(&self, base_urls: &'a [&'a str]) -> ResolveResult<Vec<Choice<'a>>> {
        let mut rng = rand::thread_rng();
        assert!(!base_urls.is_empty());
        let mut choices = Vec::<Choice>::with_capacity(base_urls.len());
        for base_url in base_urls.iter() {
            if !self.is_frozen_url(base_url)? {
                if let Some(choice) = self.make_choice(base_url, &mut rng) {
                    choices.push(choice);
                }
            }
        }
        if choices.is_empty() {
            choices.push(
                base_urls
                    .iter()
                    .filter_map(|base_url| self.make_choice(base_url, &mut rng))
                    .min_by_key(|choice| {
                        self.inner
                            .inner_data
                            .frozen_urls
                            .get(&Self::host_with_port(choice.base_url).unwrap())
                            .map(|time| time.duration_since(SystemTime::UNIX_EPOCH).unwrap())
                            .unwrap_or_else(|| Duration::from_secs(0))
                    })
                    .unwrap(),
            );
        }
        {
            let domains_manager = self.clone();
            global_thread_pool.read().unwrap().spawn(move || {
                domains_manager.try_to_persistent_if_needed();
                if !domains_manager.inner.inner_data.url_resolution_disabled {
                    domains_manager.try_to_async_refresh_resolutions_if_needed();
                }
            })
        }
        Ok(choices)
    }

    pub fn freeze_url(&self, url: &str) -> URLParseResult<()> {
        self.inner.inner_data.frozen_urls.insert(
            Self::host_with_port(url)?,
            SystemTime::now() + self.inner.inner_data.url_frozen_duration,
        );
        self.try_to_persistent_if_needed();
        Ok(())
    }

    pub fn unfreeze_urls(&self) {
        self.inner.inner_data.frozen_urls.clear();
        self.try_to_persistent_if_needed();
    }

    pub fn is_frozen_url(&self, url: &str) -> URLParseResult<bool> {
        let url = Self::host_with_port(url)?;
        match self.inner.inner_data.frozen_urls.get(&url) {
            Some(unfreeze_time) => {
                if *unfreeze_time < SystemTime::now() {
                    drop(unfreeze_time);
                    self.inner.inner_data.frozen_urls.remove(&url);
                    Ok(false)
                } else {
                    Ok(true)
                }
            }
            None => Ok(false),
        }
    }

    fn make_choice<'a>(&self, base_url: &'a str, rng: &mut ThreadRng) -> Option<Choice<'a>> {
        if self.inner.inner_data.url_resolution_disabled {
            return Some(Choice {
                base_url,
                socket_addrs: Vec::new().into(),
            });
        }
        self.resolve(base_url)
            .ok()
            .map(|mut socket_addrs| {
                // TODO: Think about IP address speed testing
                socket_addrs.shuffle(rng);
                socket_addrs
            })
            .map(|socket_addrs| Choice { base_url, socket_addrs })
    }

    fn resolve(&self, url: &str) -> ResolveResult<Box<[SocketAddr]>> {
        let url = Self::host_with_port(url)?;
        match self.inner.inner_data.resolutions.get(&url) {
            Some(resolution) => {
                if resolution.cache_deadline < SystemTime::now() {
                    Ok(resolution.socket_addrs.clone()).tap(|_| self.async_update_cache(url))
                } else {
                    Ok(resolution.socket_addrs.clone())
                }
            }
            None => self.resolve_and_update_cache(&url),
        }
    }

    fn async_update_cache(&self, url: Box<str>) {
        let domains_manager = self.clone();
        global_thread_pool.read().unwrap().spawn(move || {
            let _ = domains_manager.resolve_and_update_cache(&url);
        });
    }

    fn resolve_and_update_cache(&self, url: &str) -> ResolveResult<Box<[SocketAddr]>> {
        let mut result: Option<ResolveResult<Box<[SocketAddr]>>> = None;
        self.inner
            .inner_data
            .resolutions
            .alter(url.into(), |resolution| match resolution {
                Some(resolution) => {
                    if resolution.cache_deadline < SystemTime::now() {
                        match self.make_resolution(url) {
                            Ok(resolution) => {
                                result = Some(Ok(resolution.socket_addrs.clone()));
                                Some(resolution)
                            }
                            Err(err) => {
                                result = Some(Err(err));
                                None
                            }
                        }
                    } else {
                        result = Some(Ok(resolution.socket_addrs.clone()));
                        Some(resolution)
                    }
                }
                None => match self.make_resolution(url) {
                    Ok(resolution) => {
                        result = Some(Ok(resolution.socket_addrs.clone()));
                        Some(resolution)
                    }
                    Err(err) => {
                        result = Some(Err(err));
                        None
                    }
                },
            });
        result.unwrap()
    }

    fn make_resolution(&self, url: &str) -> ResolveResult<CachedResolutions> {
        Ok(CachedResolutions {
            socket_addrs: url.to_socket_addrs()?.collect::<Box<[_]>>(),
            cache_deadline: SystemTime::now() + self.inner.inner_data.resolutions_cache_lifetime,
        })
    }

    fn host_with_port(url: &str) -> URLParseResult<Box<str>> {
        let parsed_url = Url::parse(&url)?;

        match (parsed_url.host_str(), parsed_url.port_or_known_default()) {
            (Some(host), Some(port)) => Ok((host.to_owned() + ":" + &port.to_string()).into()),
            _ => Err(URLParseError::InvalidURL { url: url.into() }),
        }
    }

    pub fn async_resolve_config(&self, config: &Config) {
        let mut urls = HashSet::with_capacity(6);
        urls.insert(Cow::Owned(config.uc_url()));
        urls.insert(Cow::Owned(config.rs_url()));
        urls.insert(Cow::Owned(config.rsf_url()));
        urls.insert(Cow::Owned(config.api_url()));
        urls.insert(config.uplog_url().to_owned());
        self.async_resolve_urls(urls)
    }

    pub fn async_resolve_region(&self, region: &Region) {
        let mut urls = HashSet::with_capacity(100);
        region.up_urls_owned(false).into_iter().for_each(|url| {
            urls.insert(url);
        });
        region.up_urls_owned(true).into_iter().for_each(|url| {
            urls.insert(url);
        });
        region.io_urls_owned(false).into_iter().for_each(|url| {
            urls.insert(url);
        });
        region.io_urls_owned(true).into_iter().for_each(|url| {
            urls.insert(url);
        });
        region.rs_urls_owned(false).into_iter().for_each(|url| {
            urls.insert(url);
        });
        region.rs_urls_owned(true).into_iter().for_each(|url| {
            urls.insert(url);
        });
        region.rsf_urls_owned(false).into_iter().for_each(|url| {
            urls.insert(url);
        });
        region.rsf_urls_owned(true).into_iter().for_each(|url| {
            urls.insert(url);
        });
        region.api_urls_owned(false).into_iter().for_each(|url| {
            urls.insert(url);
        });
        region.api_urls_owned(true).into_iter().for_each(|url| {
            urls.insert(url);
        });
        self.async_resolve_urls(urls)
    }

    fn async_resolve_urls(&self, urls: HashSet<Cow<'static, str>>) {
        let domains_manager = self.clone();
        global_thread_pool.read().unwrap().spawn(move || {
            domains_manager.sync_resolve_urls(urls);
        });
    }

    fn try_to_async_refresh_resolutions_if_needed(&self) {
        if let Some(refresh_resolutions_interval) = self.inner.inner_data.refresh_resolutions_interval {
            let mut last_refresh_time = self.inner.last_refresh_time.lock().unwrap();
            if last_refresh_time.elapsed() > refresh_resolutions_interval {
                *last_refresh_time = Instant::now();
                self.async_refresh_resolutions_without_update_refresh_time();
            }
        }
    }

    fn async_refresh_resolutions_without_update_refresh_time(&self) {
        if self.inner.inner_data.resolutions.is_empty() {
            return;
        }
        let now = SystemTime::now();
        let to_fresh_urls = RefCell::new(HashSet::new());
        self.inner.inner_data.resolutions.retain(|url, resolution| {
            if resolution.cache_deadline <= now {
                to_fresh_urls.borrow_mut().insert(Cow::Owned(url.to_string()));
                false
            } else {
                true
            }
        });
        let to_fresh_urls = to_fresh_urls.into_inner();
        if !to_fresh_urls.is_empty() {
            self.async_resolve_urls(to_fresh_urls);
        }
    }

    fn sync_resolve_urls(&self, mut urls: HashSet<Cow<'static, str>>) {
        let mut rng = thread_rng();
        for _ in 0..self.inner.inner_data.url_resolve_retries {
            urls = urls
                .into_iter()
                .map(|url| (self.resolve(&url), url))
                .filter_map(|(result, url)| result.err().map(|_| url))
                .collect();
            if urls.is_empty() {
                break;
            } else {
                let delay_nanos = self.inner.inner_data.url_resolve_retry_delay.as_nanos() as u64;
                if delay_nanos > 0 {
                    sleep(Duration::from_nanos(rng.gen_range(delay_nanos / 2, delay_nanos)));
                }
            }
        }
    }

    #[inline]
    pub fn url_frozen_duration(&self) -> Duration {
        self.inner.inner_data.url_frozen_duration
    }

    #[inline]
    pub fn resolutions_cache_lifetime(&self) -> Duration {
        self.inner.inner_data.resolutions_cache_lifetime
    }

    #[inline]
    pub fn url_resolution_disabled(&self) -> bool {
        self.inner.inner_data.url_resolution_disabled
    }

    #[inline]
    pub fn auto_persistent_interval(&self) -> Option<Duration> {
        self.inner.inner_data.persistent_interval
    }

    #[inline]
    pub fn auto_persistent_disabled(&self) -> bool {
        self.auto_persistent_interval().is_none()
    }

    #[inline]
    pub fn url_resolve_retries(&self) -> usize {
        self.inner.inner_data.url_resolve_retries
    }

    #[inline]
    pub fn url_resolve_retry_delay(&self) -> Duration {
        self.inner.inner_data.url_resolve_retry_delay
    }

    #[inline]
    pub fn persistent_file_path(&self) -> Option<&Path> {
        self.inner.persistent_file_path.as_ref().map(|path| path.as_path())
    }

    #[allow(dead_code)]
    fn ignore() {
        assert_impl!(Send: Self);
        assert_impl!(Sync: Self);
    }
}

impl Default for DomainsManager {
    fn default() -> Self {
        DomainsManagerBuilder::default().build()
    }
}

#[derive(Debug, Clone)]
pub struct Choice<'a> {
    pub base_url: &'a str,
    pub socket_addrs: Box<[SocketAddr]>,
}

#[derive(Error, Debug)]
pub enum URLParseError {
    #[error("Invalid url: {url}")]
    InvalidURL { url: String },
    #[error("URL parse error: {0}")]
    URLParseError(#[from] url::ParseError),
}

pub type URLParseResult<T> = Result<T, URLParseError>;

#[derive(Error, Debug)]
pub enum ResolveError {
    #[error("URL Parse error: {0}")]
    URLParseError(#[from] URLParseError),
    #[error("Resolve URL error: {0}")]
    ResolveError(#[from] IOError),
}

pub type ResolveResult<T> = Result<T, ResolveError>;

#[derive(Error, Debug)]
pub enum PersistentError {
    #[error("JSON encode error: {0}")]
    JSONError(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    IOError(#[from] IOError),
}

pub type PersistentResult<T> = Result<T, PersistentError>;

#[cfg(test)]
mod tests {
    use super::*;
    use qiniu_test_utils::temp_file;
    use std::{boxed::Box, error::Error, result::Result, thread};

    #[test]
    fn test_domains_manager_in_multiple_threads() -> Result<(), Box<dyn Error>> {
        let domains_manager = DomainsManagerBuilder::default()
            .disable_url_resolution()
            .url_frozen_duration(Duration::from_secs(5))
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
        assert_eq!(choices.first().unwrap().base_url, "http://up-z0.qiniup.com");
        assert!(choices.first().unwrap().socket_addrs.len() > 0);

        let choices = domains_manager.choose(&[
            "http://up-z1.qiniup.com",
            "http://up-z2.qiniup.com",
            "http://unexisted-z3.qiniup.com",
            "http://unexisted-z4.qiniup.com",
        ])?;
        assert_eq!(choices.len(), 1);
        assert_eq!(choices.first().unwrap().base_url, "http://up-z2.qiniup.com");
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
        let inner = DomainsManagerInnerData::load_from_file(temp_path)?;
        assert!(inner.frozen_urls.contains_key("up-z0.qiniup.com:80"));
        assert!(inner.frozen_urls.contains_key("up-z1.qiniup.com:80"));
        assert!(inner.resolutions.contains_key("up-z2.qiniup.com:80"));
        assert!(!inner.resolutions.contains_key("unexisted-z3.qiniup.com:80"));
        assert!(!inner.resolutions.contains_key("unexisted-z4.qiniup.com:80"));

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
            .disable_url_resolution()
            .build();
        domains_manager.freeze_url("http://up-z0.qiniup.com")?;
        DomainsManagerInnerData::load_from_file(temp_path).unwrap_err();
        thread::sleep(Duration::from_secs(1));
        domains_manager.freeze_url("http://up-z1.qiniup.com")?;
        DomainsManagerInnerData::load_from_file(temp_path)?;
        Ok(())
    }
}
