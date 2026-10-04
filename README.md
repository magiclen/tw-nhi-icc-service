TW NHI IC Card Service
===============

[![CI](https://github.com/magiclen/tw-nhi-icc-service/actions/workflows/ci.yml/badge.svg)](https://github.com/magiclen/tw-nhi-icc-service/actions/workflows/ci.yml)

透過 HTTP API 讀取中華民國健保卡。

Read Taiwan NHI cards via HTTP API.

## 用法

#### 執行環境

###### Windows / macOS

請先安裝好您讀卡機的驅動程式。

###### Linux

需有 `pcscd` (來自 [PCSClite project](https://pcsclite.apdu.fr/))。

基於 Debian 的 Linux 發行版可用以下指令安裝：

```bash
sudo apt install pcscd
sudo systemctl enable pcscd
```

接著安裝好您讀卡機的驅動程式。

#### 開發環境

若要在 GNU/Linux 下編譯本專案，需要 `libpcsclite-dev` 套件。

基於 Debian 的 Linux 發行版可用以下指令安裝：

```bash
sudo apt install libpcsclite-dev
```

#### 命令列介面 (CLI)

```text
EXAMPLES:
tw-nhi-icc-service                                          # 啟動 HTTP 服務，監聽 127.0.0.1:12345
tw-nhi-icc-service -i 0.0.0.0 -p 8080                       # 啟動 HTTP 服務，監聽 0.0.0.0:8080
tw-nhi-icc-service --allow-origin https://his.example.com   # 只允許 https://his.example.com 的網頁存取

Usage: tw-nhi-icc-service [OPTIONS]

Options:
  -i, --interface <INTERFACE>                     要監聽的網路介面 IP [default: 127.0.0.1] [alias: --ip]
  -p, --port <PORT>                               要監聽的連接埠 [default: 12345]
      --default-ws-card-fetch-interval <SECONDS>  WebSocket 在讀卡狀態沒有變化時，重送目前狀態的預設時間間隔（秒） [default: 3] [alias: --interval]
      --allow-origin <ORIGIN>                     允許存取此服務的網頁來源（Origin），例如 https://example.com；可重複指定，沒有指定時允許所有來源。有指定時，只能透過 IP 或 localhost 連線到此服務
  -h, --help                                      Print help
  -V, --version                                   Print version
```

服務會在背景監控所有讀卡機，在插入卡片時讀取並快取，所以 HTTP API 會立即回應，WebSocket 也會在插拔卡片時立即推送。有些讀卡機的驅動程式會漏掉插拔卡事件，所以卡片插著時，服務每 3 秒會重新讀卡，確認卡片沒有被拔出或更換。

健保卡資料屬於個人資料。若網頁系統的網域是固定的，建議使用 `--allow-origin` 限制可以存取此服務的網頁來源，避免其他網站在背景讀取健保卡資料。為了防範 DNS rebinding 攻擊，有指定 `--allow-origin` 時，只能透過 IP（例如 `127.0.0.1`）或 `localhost` 連線到此服務。

#### HTTP API

啟動 HTTP 服務後，可以存取以下的端點：

* `GET /`：取得所有讀卡機目前的狀態。回應的 Content-Type 為 `application/json`。JSON 格式如下：
    ```json
    {
        "type": "snapshot",
        "status": "ok",
        "error": null,
        "readers": [
            {
                "name": "讀卡機名稱",
                "state": "nhi_card",
                "card": {
                    "card_no": "卡號",
                    "full_name": "全名",
                    "id_no": "身份證字號",
                    "birth_date": "0000-00-00",
                    "birth_date_timestamp": 0,
                    "sex": "M：男；F：女",
                    "issue_date": "0000-00-00",
                    "issue_date_timestamp": 0
                },
                "error": null
            },

            ...
        ]
    }
    ```
    * `status`：服務的狀態。
        * `ok`：PC/SC 服務可以使用。沒有任何讀卡機時，`readers` 為空陣列。
        * `pcsc_unavailable`：PC/SC 服務無法使用，`readers` 為空陣列，`error` 為 PC/SC 的錯誤名稱（例如 `NoService`）。服務會自動重試，恢復後狀態就會變回 `ok`。Windows 在沒有接任何讀卡機時，系統的智慧卡服務可能沒有啟動，也會是這個狀態。
    * `readers[].state`：讀卡機的狀態。
        * `empty`：沒有插卡。
        * `nhi_card`：讀到健保卡，資料在 `card` 欄位。
        * `unsupported_card`：有卡片，但不是健保卡（例如 SAM 卡或晶片金融卡）。
        * `error`：讀卡失敗，`error` 為 PC/SC 的錯誤名稱。例如 `SharingViolation` 代表卡片正被其他程式獨占使用，服務會自動重試。
    * `card` 與 `error` 欄位一定存在，不適用時為 `null`。
    * `full_name` 中無法以 Big5 解碼的字（例如部分罕用字）會以 `U+FFFD`（`�`）取代。
    * 時間戳記(timestamp)的單位是毫秒，代表該日期在台灣時區（UTC+8）的午夜，與伺服器的時區無關。若只需要日期，建議直接使用 `birth_date` 與 `issue_date` 欄位。
* `GET /version`：回傳此服務的版本，可用來檢驗此服務是否正常在監聽。回應的 Content-Type 為 `application/json`。JSON 格式如下：
    ```json
    {
        "major": 0,
        "minor": 3,
        "patch": 1,
        "pre": "",
        "text": "0.3.1"
    }
    ```
* `GET /ws`：**WebSocket 端點**。伺服器會以文字訊息送出與 `GET /` 相同格式的 JSON，送出的時機如下：
    * 連線建立後立即送出一次。
    * 讀卡狀態改變（例如插拔卡片、接上或移除讀卡機）時立即送出。
    * 超過 `interval` 秒都沒有送出任何訊息時，重送目前的狀態。客戶端可以據此判斷連線是否還活著，例如超過兩倍的 `interval` 都沒有收到訊息時就重新連線。

    查詢中可以代入 `interval` 欄位來設定上述的時間間隔，單位為秒，範圍為 `1` 到 `86400`，預設值由 `--default-ws-card-fetch-interval` 決定。客戶端也可以在連線時傳送秒數（範圍同上）來更改時間間隔，或是傳送 `close` 來關閉連線。服務關閉時，伺服器會送出代碼為 `1001` 的 Close frame。
* `GET /docs`：**Swagger UI**。可以在瀏覽器中查看 API 文件，並直接測試 `GET /` 與 `GET /version`。
* `GET /docs/json`：OpenAPI 3.1 文件（JSON）。

## 客戶端函式庫

* [`tw-nhi-icc`](https://github.com/magiclen/tw-nhi-icc)：在 JavaScript/TypeScript 中，讀取中華民國健保卡。

## License

[MIT](LICENSE)