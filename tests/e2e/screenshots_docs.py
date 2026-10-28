"""Captures the README and docs screenshots from the real app (`just screenshots`).

Builds a demo workspace that calls the built-in echo server ({{$localEcho}}), so the
pictures need no network and are reproducible. Writes PNGs to docs/images/.
"""
import json, os, shutil, sys, tempfile, time

sys.path.insert(0, os.path.dirname(__file__))
from wd import APP_ID, CTRL, RELEASE, Driver, data_env, restore_workspace  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../.."))
OUT = os.path.join(ROOT, "docs", "images")
os.makedirs(OUT, exist_ok=True)

tmp = tempfile.mkdtemp(prefix="ironcall-shots-")
data, ws = os.path.join(tmp, "data"), os.path.join(tmp, "Bookstore API")
os.makedirs(os.path.join(data, APP_ID))
for d in ["environments", "auth", "books", ".ironcall"]:
    os.makedirs(os.path.join(ws, d))

ENV = "01J9Z3M0A1B2C3D4E5F6G7H8E1"
IDS = {
    "auth": "01J9Z3Q0A1B2C3D4E5F6G7H8F1",
    "books": "01J9Z3Q0A1B2C3D4E5F6G7H8F2",
    "token": "01J9Z3K7T2V8Q4M6N0P5R1S3A1",
    "list": "01J9Z3K7T2V8Q4M6N0P5R1S3A2",
    "create": "01J9Z3K7T2V8Q4M6N0P5R1S3A3",
    "delete": "01J9Z3K7T2V8Q4M6N0P5R1S3A4",
    "health": "01J9Z3K7T2V8Q4M6N0P5R1S3A5",
}


def write(rel, text):
    with open(os.path.join(ws, rel), "w", newline="\n") as f:
        f.write(text)


write("ironcall.yaml", f"""schema: 'ironcall/collection@1'
id: '01J9Z3P0A1B2C3D4E5F6G7H8C1'
name: 'Bookstore API'
description: |
  Requests for the Bookstore service.
auth:
  type: 'bearer'
  token: '{{{{authToken}}}}'
order:
- '{IDS["auth"]}'
- '{IDS["books"]}'
- '{IDS["health"]}'
""")
write("environments/local.yaml", f"""schema: 'ironcall/environment@1'
id: '{ENV}'
name: 'Local'
vars:
  baseUrl: '{{{{$localEcho}}}}'
""")
write("auth/_folder.yaml", f"""schema: 'ironcall/folder@1'
id: '{IDS["auth"]}'
name: 'Auth'
order:
- '{IDS["token"]}'
""")
write("auth/get-a-token.yaml", f"""schema: 'ironcall/request@1'
id: '{IDS["token"]}'
name: 'Get a token'
method: 'POST'
url: '{{{{baseUrl}}}}/token'
auth:
  type: 'none'
extract:
- {{var: 'authToken', from: 'body', path: '$.access_token', sensitive: true}}
assert:
- {{target: 'status', op: 'eq', value: 200}}
""")
write("books/_folder.yaml", f"""schema: 'ironcall/folder@1'
id: '{IDS["books"]}'
name: 'Books'
order:
- '{IDS["list"]}'
- '{IDS["create"]}'
- '{IDS["delete"]}'
""")
write("books/list-books.yaml", f"""schema: 'ironcall/request@1'
id: '{IDS["list"]}'
name: 'List books'
method: 'GET'
url: '{{{{baseUrl}}}}/json'
params:
- {{name: 'page', value: '1', enabled: true}}
assert:
- {{target: 'status', op: 'eq', value: 200}}
- {{target: 'body', path: '$.store.books[?@.price > 50].title', op: 'eq', value: 'HTTP: The Definitive Guide'}}
- {{target: 'body', path: '$.store.open', op: 'type', value: 'boolean'}}
- {{target: 'duration', op: 'lt', value: 500}}
extract:
- {{var: 'firstBookId', from: 'body', path: '$.store.books[0].id', sensitive: false}}
""")
write("books/create-book.yaml", f"""schema: 'ironcall/request@1'
id: '{IDS["create"]}'
name: 'Create book'
method: 'POST'
url: '{{{{baseUrl}}}}/anything/books'
headers:
- {{name: 'Content-Type', value: 'application/json', enabled: true}}
body:
  type: 'json'
  content: |
    {{
      "title": "Designing Data-Intensive Applications",
      "author": "Martin Kleppmann",
      "isbn": "{{{{$uuid}}}}"
    }}
assert:
- {{target: 'status', op: 'eq', value: 200}}
- {{target: 'body', path: '$.json.author', op: 'eq', value: 'Martin Kleppmann'}}
""")
write("books/delete-book.yaml", f"""schema: 'ironcall/request@1'
id: '{IDS["delete"]}'
name: 'Delete book'
method: 'DELETE'
url: '{{{{baseUrl}}}}/anything/books/{{{{firstBookId}}}}'
assert:
- {{target: 'status', op: 'in', value: [200, 204]}}
""")
write("health.yaml", f"""schema: 'ironcall/request@1'
id: '{IDS["health"]}'
name: 'Health'
method: 'GET'
url: '{{{{baseUrl}}}}/status/200'
auth:
  type: 'none'
""")
write(".ironcall/local.yaml", f"activeEnvironment: '{ENV}'\n")
restore_workspace(data, ws, secret_backend="plaintextFile")

d = Driver(os.path.join(tmp, "logs"), env=data_env(data))


def shot(name):
    time.sleep(0.6)  # let transitions and fonts settle
    d.shot(name)
    shutil.move(os.path.join(tmp, "logs", f"{name}.png"), os.path.join(OUT, f"{name}.png"))
    print("wrote", f"docs/images/{name}.png")


def tree_item(name):
    return d.find(css=f'[role="treeitem"][aria-label="{name}"]')


def open_request(name):
    d.click(tree_item(name))
    d.wait(lambda: d.exists(xpath=f'//*[@role="tab" and contains(@aria-label, "{name}")]', timeout=1))


def send():
    d.click_on(xpath='//button[normalize-space()="Send"]')
    d.find(xpath='//*[@aria-label="Response"]//*[contains(normalize-space(), " OK")]', timeout=20)


def tab(group, label):
    d.click_on(xpath=f'//*[@role="tablist" and @aria-label="{group}"]/*[@role="tab" and starts-with(normalize-space(), "{label}")]')


def keys(combo):
    d.type_into(CTRL + combo + RELEASE, css='input[aria-label="URL"]')


def set_theme(choice):
    d.click_on(xpath='//header//button[normalize-space()="Settings"]')
    d.click_on(xpath=f'//select[@aria-label="Theme"]/option[@value="{choice}"]')
    d.click_on(xpath='//button[normalize-space()="Save"]')
    d.wait(lambda: not d.exists(css='[role="dialog"]', timeout=0.5))


try:
    d.find(css='[role="tree"]')
    # The README shows the dark theme; one light-theme picture comes last.
    set_theme("dark")
    for folder in ["Auth", "Books"]:
        d.click(tree_item(folder))

    # Chain: get a token first so later requests resolve {{authToken}}.
    open_request("Get a token")
    send()

    # 1. Main window: a JSON response.
    open_request("List books")
    send()
    shot("main")

    # 2. Tests: assertion results and extracted values.
    tab("Response", "Tests")
    tab("Request", "Tests")
    shot("tests")

    # 3. A POST with a JSON body.
    open_request("Create book")
    tab("Request", "Body")
    send()
    shot("request-body")

    # 4. Collection runner.
    d.click_on(xpath='//header//button[normalize-space()="Run"]')
    dialog = '//*[@role="dialog" and starts-with(@aria-label, "Run: ")]'
    d.find(xpath=f'{dialog}//*[contains(normalize-space(), "requests, in order.")]')
    d.click_on(xpath=f'{dialog}//button[normalize-space()="Run"]')
    d.find(css='[aria-label="Run summary"]', timeout=30)
    d.click_on(css='[aria-label="Details of List books"]')
    shot("runner")
    d.click_on(xpath=f'{dialog}//button[normalize-space()="Close"]')

    # 5. Quick open.
    keys("p")
    d.type_into("book", css='[role="combobox"][aria-label="Search requests"]')
    shot("quick-open")
    d.type_into("", css='[role="combobox"][aria-label="Search requests"]')  # Escape

    # 6. Variables quick look.
    d.click_on(css='[aria-label="Variables quick look"]')
    shot("variables")
    d.click_on(css='[aria-label="Variables quick look"]')

    # 7. Light theme.
    set_theme("light")
    open_request("List books")
    send()
    shot("light")
finally:
    d.quit()
