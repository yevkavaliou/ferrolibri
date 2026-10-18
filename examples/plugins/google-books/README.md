# Google Books

Searches the [Google Books API](https://developers.google.com/books/docs/v1/using) (`volumes.list`) by title, author or ISBN. A result has the title, authors, publisher, year, ISBN, language, categories, the description as plain text, a cover and a link to the book's page on Google Books. The plugin offers **no downloads**.

```text
cargo run -p ferrolibri-plugins --bin ferrolibri-plugin-check -- examples/plugins/google-books --query "dracula" --setting api_key=YOUR_KEY
```

* **API key (optional).** Google limits anonymous use of the API with a quota shared by everyone. When this plugin was written (18 October 2026) the shared quota was exhausted and Google answered every anonymous request with status 429; the plugin then says so and asks for a key. Create your own key in the Google Cloud console (enable the *Books API*) and enter it in the plugin's settings.
  * The key is stored as plain text in the plugin settings file of the app and is sent to `www.googleapis.com` in the request address. Use a key restricted to the Books API, and do not share the settings file.
* The plugin's mapping was tested against a response written from the API documentation, **not** against the live service: with no key the live service could not be reached. Report differences you find.
* The covers are `books.google.com/books/content` pictures; the plugin asks for `https` and drops the page-curl effect Google adds.
