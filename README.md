# chat

Open a terminal window for each path: chat/server/src and chat/client/src

Run cargo run in each window (run multiple windows for multiple clients)

Server

```
../rust/chat/server/src$ cargo run
2022-12-02T01:55:19.494333Z  INFO server: Server starting.. "127.0.0.1:4321"
2022-12-02T01:55:26.255601Z  INFO server: Server received new client connection 127.0.0.1:41648
2022-12-02T01:55:47.199650Z  INFO server: Remote 127.0.0.1:41648 has closed connection
```

Client

```
../rust/chat/client/src$ cargo run
2022-12-02T01:55:26.255083Z  INFO ThreadId(01) client: Client starting, connecting to server "127.0.0.1:4321"
apples
2022-12-02T01:55:36.870745Z  INFO ThreadId(03) client: Server: "apples"
banan
2022-12-02T01:55:41.785728Z  INFO ThreadId(02) client: Server: "banan"
```
