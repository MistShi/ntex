//! Various helpers for Actix applications to use during testing.
use std::convert::TryFrom;
use std::error::Error;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::mpsc;
use std::{fmt, net, thread, time};

use actix_codec::{AsyncRead, AsyncWrite, Framed};
use actix_router::{Path, ResourceDef, Url};
use actix_rt::{time::delay_for, System};
use actix_server::Server;
use bytes::{Bytes, BytesMut};
use futures::future::ok;
use futures::stream::{Stream, StreamExt};
use net2::TcpBuilder;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json;

#[cfg(feature = "cookie")]
use coo_kie::Cookie;

use crate::http::body::{Body, MessageBody};
use crate::http::client::error::WsClientError;
use crate::http::client::{Client, ClientRequest, ClientResponse, Connector};
use crate::http::error::{HttpError, PayloadError, ResponseError};
use crate::http::header::{HeaderName, IntoHeaderValue, CONTENT_TYPE};
use crate::http::test::TestRequest as HttpTestRequest;
use crate::http::{
    Extensions, HttpService, Method, Payload, Request, StatusCode, Uri, Version,
};
use crate::{map_config, IntoService, IntoServiceFactory, Service, ServiceFactory};

use crate::web::config::AppConfig;
use crate::web::error::{DefaultError, WebError};
use crate::web::request::HttpRequestPool;
use crate::web::rmap::ResourceMap;
use crate::web::service::{WebRequest, WebResponse};
use crate::web::{HttpRequest, HttpResponse};

/// Create service that always responds with `HttpResponse::Ok()`
pub fn ok_service() -> impl Service<
    Request = WebRequest,
    Response = WebResponse<Body>,
    Error = WebError<DefaultError>,
> {
    default_service(StatusCode::OK)
}

/// Create service that responds with response with specified status code
pub fn default_service(
    status_code: StatusCode,
) -> impl Service<
    Request = WebRequest,
    Response = WebResponse<Body>,
    Error = WebError<DefaultError>,
> {
    (move |req: WebRequest| {
        ok(req.into_response(HttpResponse::build(status_code).finish()))
    })
    .into_service()
}

/// This method accepts application builder instance, and constructs
/// service.
///
/// ```rust
/// use ntex::service::Service;
/// use ntex::http::StatusCode;
/// use ntex::web::{self, test, App, HttpResponse};
///
/// #[ntex::test]
/// async fn test_init_service() {
///     let mut app = test::init_service(
///         App::new()
///             .service(web::resource("/test").to(|| async { HttpResponse::Ok() }))
///     ).await;
///
///     // Create request object
///     let req = test::TestRequest::with_uri("/test").to_request();
///
///     // Execute application
///     let resp = app.call(req).await.unwrap();
///     assert_eq!(resp.status(), StatusCode::OK);
/// }
/// ```
pub async fn init_service<R, S, B, E>(
    app: R,
) -> impl Service<Request = Request, Response = WebResponse<B>, Error = E>
where
    R: IntoServiceFactory<S>,
    S: ServiceFactory<
        Config = AppConfig,
        Request = Request,
        Response = WebResponse<B>,
        Error = E,
    >,
    S::InitError: std::fmt::Debug,
{
    let srv = app.into_factory();
    srv.new_service(AppConfig::default()).await.unwrap()
}

/// Calls service and waits for response future completion.
///
/// ```rust
/// use ntex::http::StatusCode;
/// use ntex::web::{self, test, App, HttpResponse};
///
/// #[ntex::test]
/// async fn test_response() {
///     let mut app = test::init_service(
///         App::new()
///             .service(web::resource("/test").to(|| async {
///                 HttpResponse::Ok()
///             }))
///     ).await;
///
///     // Create request object
///     let req = test::TestRequest::with_uri("/test").to_request();
///
///     // Call application
///     let resp = test::call_service(&mut app, req).await;
///     assert_eq!(resp.status(), StatusCode::OK);
/// }
/// ```
pub async fn call_service<S, R, B, E>(app: &mut S, req: R) -> S::Response
where
    S: Service<Request = R, Response = WebResponse<B>, Error = E>,
    E: std::fmt::Debug,
{
    app.call(req).await.unwrap()
}

/// Helper function that returns a response body of a TestRequest
///
/// ```rust
/// use bytes::Bytes;
/// use ntex::http::header;
/// use ntex::web::{self, test, App, HttpResponse};
///
/// #[ntex::test]
/// async fn test_index() {
///     let mut app = test::init_service(
///         App::new().service(
///             web::resource("/index.html")
///                 .route(web::post().to(|| async {
///                     HttpResponse::Ok().body("welcome!")
///                 })))
///     ).await;
///
///     let req = test::TestRequest::post()
///         .uri("/index.html")
///         .header(header::CONTENT_TYPE, "application/json")
///         .to_request();
///
///     let result = test::read_response(&mut app, req).await;
///     assert_eq!(result, Bytes::from_static(b"welcome!"));
/// }
/// ```
pub async fn read_response<S, B, E>(app: &mut S, req: Request) -> Bytes
where
    S: Service<Request = Request, Response = WebResponse<B>, Error = WebError<E>>,
    B: MessageBody,
{
    let mut resp = app
        .call(req)
        .await
        .unwrap_or_else(|_| panic!("read_response failed at application call"));

    let mut body = resp.take_body();
    let mut bytes = BytesMut::new();
    while let Some(item) = body.next().await {
        bytes.extend_from_slice(&item.unwrap());
    }
    bytes.freeze()
}

/// Helper function that returns a response body of a WebResponse.
///
/// ```rust
/// use bytes::Bytes;
/// use ntex::http::header;
/// use ntex::web::{self, test, App, HttpResponse};
///
/// #[ntex::test]
/// async fn test_index() {
///     let mut app = test::init_service(
///         App::new().service(
///             web::resource("/index.html")
///                 .route(web::post().to(|| async {
///                     HttpResponse::Ok().body("welcome!")
///                 })))
///     ).await;
///
///     let req = test::TestRequest::post()
///         .uri("/index.html")
///         .header(header::CONTENT_TYPE, "application/json")
///         .to_request();
///
///     let resp = test::call_service(&mut app, req).await;
///     let result = test::read_body(resp);
///     assert_eq!(result, Bytes::from_static(b"welcome!"));
/// }
/// ```
pub async fn read_body<B>(mut res: WebResponse<B>) -> Bytes
where
    B: MessageBody,
{
    let mut body = res.take_body();
    let mut bytes = BytesMut::new();
    while let Some(item) = body.next().await {
        bytes.extend_from_slice(&item.unwrap());
    }
    bytes.freeze()
}

pub async fn load_stream<S>(mut stream: S) -> Result<Bytes, Box<dyn Error>>
where
    S: Stream<Item = Result<Bytes, Box<dyn Error>>> + Unpin,
{
    let mut data = BytesMut::new();
    while let Some(item) = stream.next().await {
        data.extend_from_slice(&item?);
    }
    Ok(data.freeze())
}

/// Helper function that returns a deserialized response body of a TestRequest
///
/// ```rust
/// use ntex::http::header;
/// use ntex::web::{self, test, App, HttpResponse};
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Serialize, Deserialize)]
/// pub struct Person {
///     id: String,
///     name: String
/// }
///
/// #[ntex::test]
/// async fn test_add_person() {
///     let mut app = test::init_service(
///         App::new().service(
///             web::resource("/people")
///                 .route(web::post().to(|person: web::Json<Person>| async {
///                     HttpResponse::Ok()
///                         .json(person.into_inner())})
///                     ))
///     ).await;
///
///     let payload = r#"{"id":"12345","name":"User name"}"#.as_bytes();
///
///     let req = test::TestRequest::post()
///         .uri("/people")
///         .header(header::CONTENT_TYPE, "application/json")
///         .set_payload(payload)
///         .to_request();
///
///     let result: Person = test::read_response_json(&mut app, req).await;
/// }
/// ```
pub async fn read_response_json<S, B, T, E>(app: &mut S, req: Request) -> T
where
    S: Service<Request = Request, Response = WebResponse<B>, Error = WebError<E>>,
    B: MessageBody,
    T: DeserializeOwned,
{
    let body = read_response(app, req).await;

    serde_json::from_slice(&body)
        .unwrap_or_else(|_| panic!("read_response_json failed during deserialization"))
}

/// Test `Request` builder.
///
/// For unit testing, actix provides a request builder type and a simple handler runner. TestRequest implements a builder-like pattern.
/// You can generate various types of request via TestRequest's methods:
///  * `TestRequest::to_request` creates `actix_http::Request` instance.
///  * `TestRequest::to_srv_request` creates `WebRequest` instance, which is used for testing middlewares and chain adapters.
///  * `TestRequest::to_srv_response` creates `WebResponse` instance.
///  * `TestRequest::to_http_request` creates `HttpRequest` instance, which is used for testing handlers.
///
/// ```rust
/// use ntex::http::{header, StatusCode, HttpMessage};
/// use ntex::web::{self, test, HttpRequest, HttpResponse};
///
/// async fn index(req: HttpRequest) -> HttpResponse {
///     if let Some(hdr) = req.headers().get(header::CONTENT_TYPE) {
///         HttpResponse::Ok().into()
///     } else {
///         HttpResponse::BadRequest().into()
///     }
/// }
///
/// #[test]
/// fn test_index() {
///     let req = test::TestRequest::with_header("content-type", "text/plain")
///         .to_http_request();
///
///     let resp = index(req).await.unwrap();
///     assert_eq!(resp.status(), StatusCode::OK);
///
///     let req = test::TestRequest::default().to_http_request();
///     let resp = index(req).await.unwrap();
///     assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
/// }
/// ```
pub struct TestRequest {
    req: HttpTestRequest,
    rmap: ResourceMap,
    config: AppConfig,
    path: Path<Url>,
    peer_addr: Option<SocketAddr>,
    app_data: Extensions,
}

impl Default for TestRequest {
    fn default() -> TestRequest {
        TestRequest {
            req: HttpTestRequest::default(),
            rmap: ResourceMap::new(ResourceDef::new("")),
            config: AppConfig::default(),
            path: Path::new(Url::new(Uri::default())),
            peer_addr: None,
            app_data: Extensions::new(),
        }
    }
}

#[allow(clippy::wrong_self_convention)]
impl TestRequest {
    /// Create TestRequest and set request uri
    pub fn with_uri(path: &str) -> TestRequest {
        TestRequest::default().uri(path)
    }

    /// Create TestRequest and set header
    pub fn with_header<K, V>(key: K, value: V) -> TestRequest
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<HttpError>,
        V: IntoHeaderValue,
    {
        TestRequest::default().header(key, value)
    }

    /// Create TestRequest and set method to `Method::GET`
    pub fn get() -> TestRequest {
        TestRequest::default().method(Method::GET)
    }

    /// Create TestRequest and set method to `Method::POST`
    pub fn post() -> TestRequest {
        TestRequest::default().method(Method::POST)
    }

    /// Create TestRequest and set method to `Method::PUT`
    pub fn put() -> TestRequest {
        TestRequest::default().method(Method::PUT)
    }

    /// Create TestRequest and set method to `Method::PATCH`
    pub fn patch() -> TestRequest {
        TestRequest::default().method(Method::PATCH)
    }

    /// Create TestRequest and set method to `Method::DELETE`
    pub fn delete() -> TestRequest {
        TestRequest::default().method(Method::DELETE)
    }

    /// Set HTTP version of this request
    pub fn version(mut self, ver: Version) -> Self {
        self.req.version(ver);
        self
    }

    /// Set HTTP method of this request
    pub fn method(mut self, meth: Method) -> Self {
        self.req.method(meth);
        self
    }

    /// Set HTTP Uri of this request
    pub fn uri(mut self, path: &str) -> Self {
        self.req.uri(path);
        self
    }

    /// Set a header
    pub fn header<K, V>(mut self, key: K, value: V) -> Self
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<HttpError>,
        V: IntoHeaderValue,
    {
        self.req.header(key, value);
        self
    }

    #[cfg(feature = "cookie")]
    /// Set cookie for this request
    pub fn cookie(mut self, cookie: Cookie<'_>) -> Self {
        self.req.cookie(cookie);
        self
    }

    /// Set request path pattern parameter
    pub fn param(mut self, name: &'static str, value: &'static str) -> Self {
        self.path.add_static(name, value);
        self
    }

    /// Set peer addr
    pub fn peer_addr(mut self, addr: SocketAddr) -> Self {
        self.peer_addr = Some(addr);
        self
    }

    /// Set request payload
    pub fn set_payload<B: Into<Bytes>>(mut self, data: B) -> Self {
        self.req.set_payload(data);
        self
    }

    /// Serialize `data` to a URL encoded form and set it as the request payload. The `Content-Type`
    /// header is set to `application/x-www-form-urlencoded`.
    pub fn set_form<T: Serialize>(mut self, data: &T) -> Self {
        let bytes = serde_urlencoded::to_string(data)
            .expect("Failed to serialize test data as a urlencoded form");
        self.req.set_payload(bytes);
        self.req
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded");
        self
    }

    /// Serialize `data` to JSON and set it as the request payload. The `Content-Type` header is
    /// set to `application/json`.
    pub fn set_json<T: Serialize>(mut self, data: &T) -> Self {
        let bytes =
            serde_json::to_string(data).expect("Failed to serialize test data to json");
        self.req.set_payload(bytes);
        self.req.header(CONTENT_TYPE, "application/json");
        self
    }

    /// Set application data. This is equivalent of `App::data()` method
    /// for testing purpose.
    pub fn data<T: 'static>(mut self, data: T) -> Self {
        self.app_data.insert(data);
        self
    }

    #[cfg(test)]
    /// Set request config
    pub(crate) fn rmap(mut self, rmap: ResourceMap) -> Self {
        self.rmap = rmap;
        self
    }

    /// Complete request creation and generate `Request` instance
    pub fn to_request(mut self) -> Request {
        let mut req = self.req.finish();
        req.head_mut().peer_addr = self.peer_addr;
        req
    }

    /// Complete request creation and generate `WebRequest` instance
    pub fn to_srv_request(mut self) -> WebRequest {
        let (mut head, payload) = self.req.finish().into_parts();
        head.peer_addr = self.peer_addr;
        self.path.get_mut().update(&head.uri);

        WebRequest::new(HttpRequest::new(
            self.path,
            head,
            payload,
            Rc::new(self.rmap),
            self.config.clone(),
            Rc::new(self.app_data),
            HttpRequestPool::create(),
        ))
    }

    /// Complete request creation and generate `WebResponse` instance
    pub fn to_srv_response<B>(self, res: HttpResponse<B>) -> WebResponse<B> {
        self.to_srv_request().into_response(res)
    }

    /// Complete request creation and generate `HttpRequest` instance
    pub fn to_http_request(mut self) -> HttpRequest {
        let (mut head, payload) = self.req.finish().into_parts();
        head.peer_addr = self.peer_addr;
        self.path.get_mut().update(&head.uri);

        HttpRequest::new(
            self.path,
            head,
            payload,
            Rc::new(self.rmap),
            self.config.clone(),
            Rc::new(self.app_data),
            HttpRequestPool::create(),
        )
    }

    /// Complete request creation and generate `HttpRequest` and `Payload` instances
    pub fn to_http_parts(mut self) -> (HttpRequest, Payload) {
        let (mut head, payload) = self.req.finish().into_parts();
        head.peer_addr = self.peer_addr;
        self.path.get_mut().update(&head.uri);

        let req = HttpRequest::new(
            self.path,
            head,
            Payload::None,
            Rc::new(self.rmap),
            self.config.clone(),
            Rc::new(self.app_data),
            HttpRequestPool::create(),
        );

        (req, payload)
    }
}

/// Start test server with default configuration
///
/// Test server is very simple server that simplify process of writing
/// integration tests cases for actix web applications.
///
/// # Examples
///
/// ```rust
/// use ntex::web::{self, test, App, HttpResponse};
///
/// async fn my_handler() -> Result<HttpResponse, std::io::Error> {
///     Ok(HttpResponse::Ok().into())
/// }
///
/// #[ntex::test]
/// async fn test_example() {
///     let mut srv = test::start(
///         || App::new().service(
///                 web::resource("/").to(my_handler))
///     );
///
///     let req = srv.get("/");
///     let response = req.send().await.unwrap();
///     assert!(response.status().is_success());
/// }
/// ```
pub fn start<F, I, S, B>(factory: F) -> TestServer
where
    F: Fn() -> I + Send + Clone + 'static,
    I: IntoServiceFactory<S>,
    S: ServiceFactory<Config = AppConfig, Request = Request> + 'static,
    S::Error: ResponseError + 'static,
    S::InitError: fmt::Debug,
    S::Response: Into<HttpResponse<B>> + 'static,
    <S::Service as Service>::Future: 'static,
    B: MessageBody + 'static,
{
    start_with(TestServerConfig::default(), factory)
}

/// Start test server with custom configuration
///
/// Test server could be configured in different ways, for details check
/// `TestServerConfig` docs.
///
/// # Examples
///
/// ```rust
/// use ntex::web::{self, test, App, HttpResponse};
///
/// async fn my_handler() -> HttpResponse {
///     HttpResponse::Ok().into()
/// }
///
/// #[ntex::test]
/// async fn test_example() {
///     let mut srv = test::start_with(test::config().h1(), ||
///         App::new().service(web::resource("/").to(my_handler))
///     );
///
///     let req = srv.get("/");
///     let response = req.send().await.unwrap();
///     assert!(response.status().is_success());
/// }
/// ```
pub fn start_with<F, I, S, B>(cfg: TestServerConfig, factory: F) -> TestServer
where
    F: Fn() -> I + Send + Clone + 'static,
    I: IntoServiceFactory<S>,
    S: ServiceFactory<Config = AppConfig, Request = Request> + 'static,
    S::Error: ResponseError + 'static,
    S::InitError: fmt::Debug,
    S::Response: Into<HttpResponse<B>> + 'static,
    <S::Service as Service>::Future: 'static,
    B: MessageBody + 'static,
{
    let (tx, rx) = mpsc::channel();

    let ssl = match cfg.stream {
        StreamType::Tcp => false,
        #[cfg(feature = "openssl")]
        StreamType::Openssl(_) => true,
        #[cfg(feature = "rustls")]
        StreamType::Rustls(_) => true,
    };

    // run server in separate thread
    thread::spawn(move || {
        let sys = System::new("actix-test-server");
        let tcp = net::TcpListener::bind("127.0.0.1:0").unwrap();
        let local_addr = tcp.local_addr().unwrap();
        let factory = factory.clone();
        let cfg = cfg.clone();
        let ctimeout = cfg.client_timeout;
        let builder = Server::build().workers(1).disable_signals();

        let srv = match cfg.stream {
            StreamType::Tcp => match cfg.tp {
                HttpVer::Http1 => builder.listen("test", tcp, move || {
                    let cfg =
                        AppConfig::new(false, local_addr, format!("{}", local_addr));
                    HttpService::build()
                        .client_timeout(ctimeout)
                        .h1(map_config(factory(), move |_| cfg.clone()))
                        .tcp()
                }),
                HttpVer::Http2 => builder.listen("test", tcp, move || {
                    let cfg =
                        AppConfig::new(false, local_addr, format!("{}", local_addr));
                    HttpService::build()
                        .client_timeout(ctimeout)
                        .h2(map_config(factory(), move |_| cfg.clone()))
                        .tcp()
                }),
                HttpVer::Both => builder.listen("test", tcp, move || {
                    let cfg =
                        AppConfig::new(false, local_addr, format!("{}", local_addr));
                    HttpService::build()
                        .client_timeout(ctimeout)
                        .finish(map_config(factory(), move |_| cfg.clone()))
                        .tcp()
                }),
            },
            #[cfg(feature = "openssl")]
            StreamType::Openssl(acceptor) => match cfg.tp {
                HttpVer::Http1 => builder.listen("test", tcp, move || {
                    let cfg =
                        AppConfig::new(true, local_addr, format!("{}", local_addr));
                    HttpService::build()
                        .client_timeout(ctimeout)
                        .h1(map_config(factory(), move |_| cfg.clone()))
                        .openssl(acceptor.clone())
                }),
                HttpVer::Http2 => builder.listen("test", tcp, move || {
                    let cfg =
                        AppConfig::new(true, local_addr, format!("{}", local_addr));
                    HttpService::build()
                        .client_timeout(ctimeout)
                        .h2(map_config(factory(), move |_| cfg.clone()))
                        .openssl(acceptor.clone())
                }),
                HttpVer::Both => builder.listen("test", tcp, move || {
                    let cfg =
                        AppConfig::new(true, local_addr, format!("{}", local_addr));
                    HttpService::build()
                        .client_timeout(ctimeout)
                        .finish(map_config(factory(), move |_| cfg.clone()))
                        .openssl(acceptor.clone())
                }),
            },
            #[cfg(feature = "rustls")]
            StreamType::Rustls(config) => match cfg.tp {
                HttpVer::Http1 => builder.listen("test", tcp, move || {
                    let cfg =
                        AppConfig::new(true, local_addr, format!("{}", local_addr));
                    HttpService::build()
                        .client_timeout(ctimeout)
                        .h1(map_config(factory(), move |_| cfg.clone()))
                        .rustls(config.clone())
                }),
                HttpVer::Http2 => builder.listen("test", tcp, move || {
                    let cfg =
                        AppConfig::new(true, local_addr, format!("{}", local_addr));
                    HttpService::build()
                        .client_timeout(ctimeout)
                        .h2(map_config(factory(), move |_| cfg.clone()))
                        .rustls(config.clone())
                }),
                HttpVer::Both => builder.listen("test", tcp, move || {
                    let cfg =
                        AppConfig::new(true, local_addr, format!("{}", local_addr));
                    HttpService::build()
                        .client_timeout(ctimeout)
                        .finish(map_config(factory(), move |_| cfg.clone()))
                        .rustls(config.clone())
                }),
            },
        }
        .unwrap()
        .start();

        tx.send((System::current(), srv, local_addr)).unwrap();
        sys.run()
    });

    let (system, server, addr) = rx.recv().unwrap();

    let client = {
        let connector = {
            #[cfg(feature = "openssl")]
            {
                use open_ssl::ssl::{SslConnector, SslMethod, SslVerifyMode};

                let mut builder = SslConnector::builder(SslMethod::tls()).unwrap();
                builder.set_verify(SslVerifyMode::NONE);
                let _ = builder
                    .set_alpn_protos(b"\x02h2\x08http/1.1")
                    .map_err(|e| log::error!("Can not set alpn protocol: {:?}", e));
                Connector::new()
                    .conn_lifetime(time::Duration::from_secs(0))
                    .timeout(time::Duration::from_millis(30000))
                    .ssl(builder.build())
                    .finish()
            }
            #[cfg(not(feature = "openssl"))]
            {
                Connector::new()
                    .conn_lifetime(time::Duration::from_secs(0))
                    .timeout(time::Duration::from_millis(30000))
                    .finish()
            }
        };

        Client::build().connector(connector).finish()
    };

    TestServer {
        ssl,
        addr,
        client,
        system,
        server,
    }
}

#[derive(Clone)]
pub struct TestServerConfig {
    tp: HttpVer,
    stream: StreamType,
    client_timeout: u64,
}

#[derive(Clone)]
enum HttpVer {
    Http1,
    Http2,
    Both,
}

#[derive(Clone)]
enum StreamType {
    Tcp,
    #[cfg(feature = "openssl")]
    Openssl(open_ssl::ssl::SslAcceptor),
    #[cfg(feature = "rustls")]
    Rustls(rust_tls::ServerConfig),
}

impl Default for TestServerConfig {
    fn default() -> Self {
        TestServerConfig::new()
    }
}

/// Create default test server config
pub fn config() -> TestServerConfig {
    TestServerConfig::new()
}

impl TestServerConfig {
    /// Create default server configuration
    pub(crate) fn new() -> TestServerConfig {
        TestServerConfig {
            tp: HttpVer::Both,
            stream: StreamType::Tcp,
            client_timeout: 5000,
        }
    }

    /// Start http/1.1 server only
    pub fn h1(mut self) -> Self {
        self.tp = HttpVer::Http1;
        self
    }

    /// Start http/2 server only
    pub fn h2(mut self) -> Self {
        self.tp = HttpVer::Http2;
        self
    }

    /// Start openssl server
    #[cfg(feature = "openssl")]
    pub fn openssl(mut self, acceptor: open_ssl::ssl::SslAcceptor) -> Self {
        self.stream = StreamType::Openssl(acceptor);
        self
    }

    /// Start rustls server
    #[cfg(feature = "rustls")]
    pub fn rustls(mut self, config: rust_tls::ServerConfig) -> Self {
        self.stream = StreamType::Rustls(config);
        self
    }

    /// Set server client timeout in milliseconds for first request.
    pub fn client_timeout(mut self, val: u64) -> Self {
        self.client_timeout = val;
        self
    }
}

/// Get first available unused address
pub fn unused_addr() -> net::SocketAddr {
    let addr: net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let socket = TcpBuilder::new_v4().unwrap();
    socket.bind(&addr).unwrap();
    socket.reuse_address(true).unwrap();
    let tcp = socket.to_tcp_listener().unwrap();
    tcp.local_addr().unwrap()
}

/// Test server controller
pub struct TestServer {
    addr: net::SocketAddr,
    client: Client,
    system: actix_rt::System,
    ssl: bool,
    server: Server,
}

impl TestServer {
    /// Construct test server url
    pub fn addr(&self) -> net::SocketAddr {
        self.addr
    }

    /// Construct test server url
    pub fn url(&self, uri: &str) -> String {
        let scheme = if self.ssl { "https" } else { "http" };

        if uri.starts_with('/') {
            format!("{}://localhost:{}{}", scheme, self.addr.port(), uri)
        } else {
            format!("{}://localhost:{}/{}", scheme, self.addr.port(), uri)
        }
    }

    /// Create `GET` request
    pub fn get<S: AsRef<str>>(&self, path: S) -> ClientRequest {
        self.client.get(self.url(path.as_ref()).as_str())
    }

    /// Create `POST` request
    pub fn post<S: AsRef<str>>(&self, path: S) -> ClientRequest {
        self.client.post(self.url(path.as_ref()).as_str())
    }

    /// Create `HEAD` request
    pub fn head<S: AsRef<str>>(&self, path: S) -> ClientRequest {
        self.client.head(self.url(path.as_ref()).as_str())
    }

    /// Create `PUT` request
    pub fn put<S: AsRef<str>>(&self, path: S) -> ClientRequest {
        self.client.put(self.url(path.as_ref()).as_str())
    }

    /// Create `PATCH` request
    pub fn patch<S: AsRef<str>>(&self, path: S) -> ClientRequest {
        self.client.patch(self.url(path.as_ref()).as_str())
    }

    /// Create `DELETE` request
    pub fn delete<S: AsRef<str>>(&self, path: S) -> ClientRequest {
        self.client.delete(self.url(path.as_ref()).as_str())
    }

    /// Create `OPTIONS` request
    pub fn options<S: AsRef<str>>(&self, path: S) -> ClientRequest {
        self.client.options(self.url(path.as_ref()).as_str())
    }

    /// Connect to test http server
    pub fn request<S: AsRef<str>>(&self, method: Method, path: S) -> ClientRequest {
        self.client.request(method, path.as_ref())
    }

    pub async fn load_body<S>(
        &mut self,
        mut response: ClientResponse<S>,
    ) -> Result<Bytes, PayloadError>
    where
        S: Stream<Item = Result<Bytes, PayloadError>> + Unpin + 'static,
    {
        response.body().limit(10_485_760).await
    }

    /// Connect to websocket server at a given path
    pub async fn ws_at(
        &mut self,
        path: &str,
    ) -> Result<Framed<impl AsyncRead + AsyncWrite, crate::ws::Codec>, WsClientError>
    {
        let url = self.url(path);
        let connect = self.client.ws(url).connect();
        connect.await.map(|(_, framed)| framed)
    }

    /// Connect to a websocket server
    pub async fn ws(
        &mut self,
    ) -> Result<Framed<impl AsyncRead + AsyncWrite, crate::ws::Codec>, WsClientError>
    {
        self.ws_at("/").await
    }

    /// Gracefully stop http server
    pub async fn stop(self) {
        self.server.stop(true).await;
        self.system.stop();
        delay_for(time::Duration::from_millis(100)).await;
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.system.stop()
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::http::header;
    use crate::http::HttpMessage;
    use crate::web::{self, App, Data, Error, HttpResponse, Responder};

    #[actix_rt::test]
    async fn test_basics() {
        let req = TestRequest::with_header(header::CONTENT_TYPE, "application/json")
            .version(Version::HTTP_2)
            .header(header::DATE, "some date")
            .param("test", "123")
            .data(Data::new(20u64))
            .peer_addr("127.0.0.1:8081".parse().unwrap())
            .to_http_request();
        assert!(req.headers().contains_key(header::CONTENT_TYPE));
        assert!(req.headers().contains_key(header::DATE));
        assert_eq!(
            req.head().peer_addr,
            Some("127.0.0.1:8081".parse().unwrap())
        );
        assert_eq!(&req.match_info()["test"], "123");
        assert_eq!(req.version(), Version::HTTP_2);
        let data = req.app_data::<Data<u64>>().unwrap();
        assert_eq!(*data.get_ref(), 20);
    }

    #[actix_rt::test]
    async fn test_request_methods() {
        let mut app = init_service(
            App::new().service(
                web::resource("/index.html")
                    .route(web::put().to(|| async { HttpResponse::Ok().body("put!") }))
                    .route(
                        web::patch().to(|| async { HttpResponse::Ok().body("patch!") }),
                    )
                    .route(
                        web::delete()
                            .to(|| async { HttpResponse::Ok().body("delete!") }),
                    ),
            ),
        )
        .await;

        let put_req = TestRequest::put()
            .uri("/index.html")
            .header(header::CONTENT_TYPE, "application/json")
            .to_request();

        let result = read_response(&mut app, put_req).await;
        assert_eq!(result, Bytes::from_static(b"put!"));

        let patch_req = TestRequest::patch()
            .uri("/index.html")
            .header(header::CONTENT_TYPE, "application/json")
            .to_request();

        let result = read_response(&mut app, patch_req).await;
        assert_eq!(result, Bytes::from_static(b"patch!"));

        let delete_req = TestRequest::delete().uri("/index.html").to_request();
        let result = read_response(&mut app, delete_req).await;
        assert_eq!(result, Bytes::from_static(b"delete!"));
    }

    #[actix_rt::test]
    async fn test_response() {
        let mut app =
            init_service(App::new().service(web::resource("/index.html").route(
                web::post().to(|| async { HttpResponse::Ok().body("welcome!") }),
            )))
            .await;

        let req = TestRequest::post()
            .uri("/index.html")
            .header(header::CONTENT_TYPE, "application/json")
            .to_request();

        let result = read_response(&mut app, req).await;
        assert_eq!(result, Bytes::from_static(b"welcome!"));
    }

    #[derive(Serialize, Deserialize)]
    pub struct Person {
        id: String,
        name: String,
    }

    #[actix_rt::test]
    async fn test_response_json() {
        let mut app = init_service(App::new().service(web::resource("/people").route(
            web::post().to(|person: web::types::Json<Person>| async {
                HttpResponse::Ok().json(person.into_inner())
            }),
        )))
        .await;

        let payload = r#"{"id":"12345","name":"User name"}"#.as_bytes();

        let req = TestRequest::post()
            .uri("/people")
            .header(header::CONTENT_TYPE, "application/json")
            .set_payload(payload)
            .to_request();

        let result: Person = read_response_json(&mut app, req).await;
        assert_eq!(&result.id, "12345");
    }

    #[actix_rt::test]
    async fn test_request_response_form() {
        let mut app = init_service(App::new().service(web::resource("/people").route(
            web::post().to(|person: web::types::Form<Person>| async {
                HttpResponse::Ok().json(person.into_inner())
            }),
        )))
        .await;

        let payload = Person {
            id: "12345".to_string(),
            name: "User name".to_string(),
        };

        let req = TestRequest::post()
            .uri("/people")
            .set_form(&payload)
            .to_request();

        assert_eq!(req.content_type(), "application/x-www-form-urlencoded");

        let result: Person = read_response_json(&mut app, req).await;
        assert_eq!(&result.id, "12345");
        assert_eq!(&result.name, "User name");
    }

    #[actix_rt::test]
    async fn test_request_response_json() {
        let mut app = init_service(App::new().service(web::resource("/people").route(
            web::post().to(|person: web::types::Json<Person>| async {
                HttpResponse::Ok().json(person.into_inner())
            }),
        )))
        .await;

        let payload = Person {
            id: "12345".to_string(),
            name: "User name".to_string(),
        };

        let req = TestRequest::post()
            .uri("/people")
            .set_json(&payload)
            .to_request();

        assert_eq!(req.content_type(), "application/json");

        let result: Person = read_response_json(&mut app, req).await;
        assert_eq!(&result.id, "12345");
        assert_eq!(&result.name, "User name");
    }

    #[actix_rt::test]
    async fn test_async_with_block() {
        async fn async_with_block() -> Result<HttpResponse, Error> {
            let res = web::block(move || Some(4usize).ok_or("wrong")).await;

            match res {
                Ok(value) => Ok(HttpResponse::Ok()
                    .content_type("text/plain")
                    .body(format!("Async with block value: {}", value))),
                Err(_) => panic!("Unexpected"),
            }
        }

        let mut app = init_service(
            App::new().service(
                web::resource("/index.html")
                    .to(crate::web::dev::__assert_handler(async_with_block)),
            ),
        )
        .await;

        let req = TestRequest::post().uri("/index.html").to_request();
        let res = app.call(req).await.unwrap();
        assert!(res.status().is_success());
    }

    #[actix_rt::test]
    async fn test_server_data() {
        async fn handler(data: web::Data<usize>) -> impl Responder {
            assert_eq!(**data, 10);
            HttpResponse::Ok()
        }

        let mut app = init_service(
            App::new()
                .data(10usize)
                .service(web::resource("/index.html").to(handler)),
        )
        .await;

        let req = TestRequest::post().uri("/index.html").to_request();
        let res = app.call(req).await.unwrap();
        assert!(res.status().is_success());
    }
}