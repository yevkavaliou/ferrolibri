//! A stand-in for an OPDS catalog mirror. It reproduces what was observed with real requests to
//! such a catalog, and nothing is fetched from a real site.
//!
//! Observed (reproduced here):
//! * The search entry point `/opds/search?searchTerm=Q` answers with a menu of two links
//!   (books, authors); the results are at `searchType=books`, and books are matched by title only.
//! * Links in feeds are relative (`/opds/...`); the OpenSearch description uses scheme-relative
//!   addresses (`//host:port/...`).
//! * A download is two `302`s, the second to a sibling subdomain (`static.<catalog host>`), and
//!   ends in `200 application/zip` without `Content-Disposition`; an FB2 book is a zip with one
//!   `.fb2` file.
//! * Errors are HTML: `502` and `400` with a body of about 150 bytes.
//! * Entries carry no annotation, cover or series.
//!
//! **Synthetic (plausible, never observed):** paging with
//! `rel="next"` and `pageNumber`, the HTML "book not found" page at the end of the download
//! chain, and the `/slow` route.

use std::time::Duration;

use url::form_urlencoded::byte_serialize;

use super::server::{CATALOG_HOST, Context, Reply, Request};
use super::zipfile::single_file_zip;

pub struct Book {
    pub id: u32,
    pub title: &'static str,
    pub author: &'static str,
}

/// The catalog's books. Ids are not contiguous on purpose.
pub const BOOKS: [Book; 5] = [
    Book {
        id: 11,
        title: "Преступление и наказание",
        author: "Достоевский, Фёдор",
    },
    Book {
        id: 12,
        title: "Война и мир",
        author: "Толстой, Лев",
    },
    Book {
        id: 13,
        title: "Анна Каренина",
        author: "Толстой, Лев",
    },
    Book {
        id: 21,
        title: "Мёртвые души",
        author: "Гоголь, Николай",
    },
    Book {
        id: 31,
        title: "Отцы и дети",
        author: "Тургенев, Иван",
    },
];

/// Books per feed page (synthetic paging).
pub const PAGE_SIZE: usize = 2;

/// An id that is in no search result; its download ends in an HTML page with status `200`.
pub const MISSING_BOOK_ID: u32 = 999;

const NAVIGATION: &str = "application/atom+xml;profile=opds-catalog;kind=navigation;charset=utf-8";
const ACQUISITION: &str =
    "application/atom+xml;profile=opds-catalog;kind=acquisition;charset=utf-8";

/// The FB2 document inside a book's zip.
pub fn fb2_document(book: &Book) -> String {
    let (last, first) = book.author.split_once(", ").unwrap_or((book.author, ""));
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <FictionBook xmlns=\"http://www.gribuser.ru/xml/fictionbook/2.0\"><description>\
         <title-info><genre>prose_classic</genre><author><first-name>{first}</first-name>\
         <last-name>{last}</last-name></author><book-title>{}</book-title><lang>ru</lang>\
         </title-info></description><body><section><p>Test text.</p></section></body></FictionBook>",
        book.title
    )
}

/// The file name inside a book's zip.
pub fn fb2_name(book: &Book) -> String {
    format!("{}.fb2", book.id)
}

/// The zip a download of `book` ends with.
pub fn book_zip(book: &Book) -> Vec<u8> {
    single_file_zip(&fb2_name(book), fb2_document(book).as_bytes())
}

fn book(id: u32) -> Option<&'static Book> {
    BOOKS.iter().find(|b| b.id == id)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn encode(text: &str) -> String {
    byte_serialize(text.as_bytes()).collect()
}

fn feed(title: &str, links: &str, entries: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <feed xmlns=\"http://www.w3.org/2005/Atom\" xmlns:dc=\"http://purl.org/dc/terms/\" \
         xmlns:opds=\"http://opds-spec.org/2010/catalog\">\
         <id>tag:{CATALOG_HOST}:feed</id><title>{}</title>{links}{entries}</feed>",
        escape(title)
    )
}

const HTML_502: &str = "<html><head><title>502 Bad Gateway</title></head><body>\
                        <center><h1>502 Bad Gateway</h1></center><hr><center>nginx</center>\
                        </body></html>";
const HTML_400: &str = "<html><head><title>400 Bad Request</title></head><body>\
                        <center><h1>400 Bad Request</h1></center><hr><center>nginx</center>\
                        </body></html>";

/// The routes. Both the catalog host and `static.<catalog host>` are served by this handler;
/// the `Host` header tells them apart.
pub fn handle(request: &Request, context: &Context) -> Reply {
    if request.host.starts_with("static.") {
        return handle_static(request);
    }
    let static_host = format!("static.{CATALOG_HOST}");
    let path = request.path.as_str();
    match path {
        "/opds" | "/opds/" => root(request),
        "/opds/opensearch.xml" => Reply::ok(
            "application/opensearchdescription+xml",
            format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
                 <OpenSearchDescription xmlns=\"http://a9.com/-/spec/opensearch/1.1/\">\
                 <ShortName>Catalog</ShortName>\
                 <Url type=\"application/atom+xml;profile=opds-catalog\" \
                 template=\"//{}/opds/search?searchTerm={{searchTerms}}\"/>\
                 </OpenSearchDescription>",
                request.host_header
            ),
        ),
        "/opds/search" => search(request),
        "/error/502" => Reply::html(502, HTML_502),
        "/error/400" => Reply::html(400, HTML_400),
        "/slow" => {
            let ms = request
                .query_param("ms")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000);
            Reply::ok("text/plain", "late").delay(Duration::from_millis(ms))
        }
        _ => match download_path(path) {
            Some((id, false)) => Reply::redirect(302, &format!("/b/{id}/fb2/get")),
            Some((id, true)) => Reply::redirect(
                302,
                &context.url(&static_host, &format!("/files/{id}.fb2.zip")),
            ),
            None => Reply::html(404, "<html><body>Not found</body></html>"),
        },
    }
}

/// `/b/{id}/fb2` → `(id, false)`, `/b/{id}/fb2/get` → `(id, true)`.
fn download_path(path: &str) -> Option<(u32, bool)> {
    let rest = path.strip_prefix("/b/")?;
    let (id, tail) = rest.split_once('/')?;
    let id = id.parse().ok()?;
    match tail {
        "fb2" => Some((id, false)),
        "fb2/get" => Some((id, true)),
        _ => None,
    }
}

fn handle_static(request: &Request) -> Reply {
    let id = request
        .path
        .strip_prefix("/files/")
        .and_then(|name| name.strip_suffix(".fb2.zip"))
        .and_then(|id| id.parse::<u32>().ok());
    match id {
        // No Content-Disposition, and the type is the container's, not the book's.
        Some(id) => match book(id) {
            Some(book) => Reply::ok("application/zip", book_zip(book)),
            None => Reply::html(200, "<html><body>The book was not found.</body></html>"),
        },
        None => Reply::html(404, "<html><body>Not found</body></html>"),
    }
}

fn root(request: &Request) -> Reply {
    let links = format!(
        "<link rel=\"self\" href=\"/opds\" type=\"{NAVIGATION}\"/>\
         <link rel=\"search\" type=\"application/opensearchdescription+xml\" \
         href=\"//{}/opds/opensearch.xml\"/>\
         <link rel=\"search\" type=\"application/atom+xml\" \
         href=\"/opds/search?searchTerm={{searchTerms}}\"/>",
        request.host_header
    );
    Reply::ok(NAVIGATION, feed("Catalog", &links, ""))
}

fn search(request: &Request) -> Reply {
    let Some(term) = request.query_param("searchTerm") else {
        return Reply::html(400, HTML_400);
    };
    let encoded = encode(&term);
    match request.query_param("searchType").as_deref() {
        None => {
            // The menu: the term alone does not search, it offers two searches.
            let entries = format!(
                "<entry><id>tag:{CATALOG_HOST}:menu:books</id><title>Books</title>\
                 <link href=\"/opds/search?searchType=books&amp;searchTerm={encoded}\" \
                 type=\"{ACQUISITION}\"/></entry>\
                 <entry><id>tag:{CATALOG_HOST}:menu:authors</id><title>Authors</title>\
                 <link href=\"/opds/search?searchType=authors&amp;searchTerm={encoded}\" \
                 type=\"{NAVIGATION}\"/></entry>"
            );
            Reply::ok(NAVIGATION, feed("Search", "", &entries))
        }
        Some("books") => {
            let wanted = term.to_lowercase();
            let found: Vec<&Book> = BOOKS
                .iter()
                .filter(|b| b.title.to_lowercase().contains(&wanted))
                .collect();
            let page: usize = request
                .query_param("pageNumber")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let shown = found.iter().skip(page * PAGE_SIZE).take(PAGE_SIZE);
            let entries: String = shown.map(|b| entry(b)).collect();
            let next = if found.len() > (page + 1) * PAGE_SIZE {
                format!(
                    "<link rel=\"next\" href=\"/opds/search?searchType=books&amp;searchTerm=\
                     {encoded}&amp;pageNumber={}\" type=\"{ACQUISITION}\"/>",
                    page + 1
                )
            } else {
                String::new()
            };
            Reply::ok(ACQUISITION, feed("Books", &next, &entries))
        }
        Some(_) => Reply::ok(NAVIGATION, feed("Authors", "", "")),
    }
}

fn entry(book: &Book) -> String {
    format!(
        "<entry><id>tag:{CATALOG_HOST}:book:{id}</id><title>{title}</title>\
         <author><name>{author}</name></author><dc:language>ru</dc:language>\
         <link rel=\"http://opds-spec.org/acquisition/open-access\" \
         href=\"/b/{id}/fb2\" type=\"application/fb2+zip\"/></entry>",
        id = book.id,
        title = escape(book.title),
        author = escape(book.author),
    )
}

#[cfg(test)]
mod tests {
    use super::super::server::TestServer;
    use super::*;

    fn get(server: &TestServer, url: &str) -> crate::FetchResponse {
        server
            .session()
            .fetch(url, &[])
            .unwrap_or_else(|e| panic!("{url}: {e}"))
    }

    fn text(response: &crate::FetchResponse) -> String {
        String::from_utf8(response.body.clone()).unwrap()
    }

    #[test]
    fn the_search_entry_point_is_a_menu_of_two_links() {
        let server = TestServer::start(handle);
        let response = get(
            &server,
            &server.url("/opds/search?searchTerm=%D0%B2%D0%BE%D0%B9"),
        );
        assert_eq!(response.head.status, 200);
        let body = text(&response);
        assert_eq!(body.matches("<entry>").count(), 2);
        assert!(body.contains("searchType=books&amp;searchTerm=%D0%B2%D0%BE%D0%B9"));
        assert!(body.contains("searchType=authors"));
    }

    #[test]
    fn books_are_found_by_title_only() {
        let server = TestServer::start(handle);
        let by_title = get(
            &server,
            &server.url("/opds/search?searchType=books&searchTerm=%D0%BC%D0%B8%D1%80"),
        );
        let body = text(&by_title);
        assert_eq!(body.matches("<entry>").count(), 1);
        assert!(body.contains("Война и мир"));
        assert!(body.contains("href=\"/b/12/fb2\""));
        // "author + title" finds nothing.
        let by_both = get(
            &server,
            &server.url("/opds/search?searchType=books&searchTerm=%D0%A2%D0%BE%D0%BB%D1%81%D1%82%D0%BE%D0%B9+%D0%B2%D0%BE%D0%B9%D0%BD%D0%B0"),
        );
        assert_eq!(text(&by_both).matches("<entry>").count(), 0);
        // Entries carry no annotation, cover or series.
        for forbidden in ["<summary", "<content", "image", "series"] {
            assert!(!body.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn links_are_relative_and_the_opensearch_description_is_scheme_relative() {
        let server = TestServer::start(handle);
        let root = text(&get(&server, &server.url("/opds")));
        let host = format!("{CATALOG_HOST}:{}", server.port());
        assert!(root.contains(&format!("href=\"//{host}/opds/opensearch.xml\"")));
        assert!(root.contains("href=\"/opds/search?searchTerm={searchTerms}\""));
        let description = text(&get(&server, &server.url("/opds/opensearch.xml")));
        assert!(description.contains(&format!("template=\"//{host}/opds/search?searchTerm=")));
    }

    #[test]
    fn a_download_is_two_302s_ending_in_a_zip_without_a_file_name() {
        let server = TestServer::start(handle);
        // Look at each hop with a client that does not follow redirects: curl-like probing.
        let first = raw_get(&server, "catalog.test", "/b/12/fb2");
        assert!(first.starts_with("HTTP/1.1 302"), "{first}");
        assert!(first.contains("Location: /b/12/fb2/get"));
        let second = raw_get(&server, "catalog.test", "/b/12/fb2/get");
        assert!(second.starts_with("HTTP/1.1 302"), "{second}");
        assert!(second.contains(&format!(
            "Location: http://static.catalog.test:{}/files/12.fb2.zip",
            server.port()
        )));

        // The whole chain, followed by the HTTP layer.
        let response = get(&server, &server.url("/b/12/fb2"));
        assert_eq!(response.head.status, 200);
        assert_eq!(response.head.content_type(), Some("application/zip"));
        assert_eq!(response.head.header("content-disposition"), None);
        assert_eq!(response.head.url.host_str(), Some("static.catalog.test"));
        assert_eq!(response.body, book_zip(&BOOKS[1]));
        // The two probes above, then the three hops of the followed chain.
        assert_eq!(
            server.paths()[2..],
            ["/b/12/fb2", "/b/12/fb2/get", "/files/12.fb2.zip"]
        );
    }

    #[test]
    fn a_missing_book_ends_in_an_html_page_with_status_200() {
        let server = TestServer::start(handle);
        let response = get(&server, &server.url(&format!("/b/{MISSING_BOOK_ID}/fb2")));
        assert_eq!(response.head.status, 200);
        assert_eq!(response.head.content_type(), Some("text/html"));
    }

    #[test]
    fn errors_are_small_html_pages() {
        let server = TestServer::start(handle);
        for (code, path) in [(502, "/error/502"), (400, "/error/400")] {
            let response = get(&server, &server.url(path));
            assert_eq!(response.head.status, code);
            assert_eq!(response.head.content_type(), Some("text/html"));
            assert!(
                (100..200).contains(&response.body.len()),
                "{}",
                response.body.len()
            );
        }
    }

    #[test]
    fn long_result_lists_are_paged_with_rel_next() {
        let server = TestServer::start(handle);
        // The letter "и" is in every title: five books, two per page.
        let term = encode("и");
        let first = text(&get(
            &server,
            &server.url(&format!("/opds/search?searchType=books&searchTerm={term}")),
        ));
        assert_eq!(first.matches("<entry>").count(), 2);
        assert!(first.contains("rel=\"next\""));
        assert!(first.contains("pageNumber=1"));
        let last = text(&get(
            &server,
            &server.url(&format!(
                "/opds/search?searchType=books&searchTerm={term}&pageNumber=2"
            )),
        ));
        assert_eq!(last.matches("<entry>").count(), 1);
        assert!(!last.contains("rel=\"next\""));
    }

    fn raw_get(server: &TestServer, host: &str, path: &str) -> String {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", server.port())).unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {host}:{}\r\n\r\n",
            server.port()
        )
        .unwrap();
        let mut out = String::new();
        stream.read_to_string(&mut out).unwrap();
        out
    }
}
