# chat

Open a terminal window for each path: chat/server and chat/client

Run cargo run in each window (run multiple windows for multiple clients)

Multi-line supported with simple peer to peer chat supported (no routing or overlay)

Supports two modes: 

* Traditional, clients/server broadcast: broadcast lobby mode, with each client getting a copy of each message
* Private peer sessions: only the two peers A & B in the peer session see the messages

```rust
Usage

$ Commands: \quit, \users, \fork chatname, \switch n, \sessions
Note: fork and users command require that you are in the lobby session e.g. \lobby
$ Please input chat name:

Note
\switch shortcut is \sw
\sessions shortcut is \ss
\fork creates a private, non broadcast session (durable if main server drops)

lobby is available on startup but after a peer session, do \sw 0 or \lob or \lobby
```

Chat session - Begin

* Start with one main/rendezvous server and three clients A, B, C -- corresponding to anna, bobby, carmen

* Note rendezvous server is queried for peer client addr info upon a call to \fork

<p float="left">
  <img src='images/chat1.png' width='845' height='450'/> 
</p>


Chat session - End

* Notice ---> main server drops out (X Bye Bye X), but active peer sessions untouched (just can't go back to lobby)

* \ss or \sessions cmd is helpful to see which sessions are active, * means active session

<p float="left">
  <img src='images/chat2.png' width='845' height='450'/> 
</p>


Chat session - Multiple forks and multiple peer sessions

* Notice ---> A (anna) -> B (bobby) -> C (carmen) -> A (anna)

* anna can privately chat with either bobby or carmen

* bobby can privately chat with either anna or carmen

* carmen can privately chat with either anna or bobby

<p float="left">
  <img src='images/chat3.png' width='845' height='450'/> 
</p>


Lastly,

* P-> means peer session in A mode (e.g. current user initiated it, e.g. sent the \fork)

* P~> means peer session in B mode (e.g. current user received a peer session request from its local peer server)

* When \fork-ing, can't fork again to same peer name with an already active session! Can't also self fork!

* For demo purposes only supports max 4 peer servers on the same node (in production, each peer server would be its own node)

* Handles duplicate names somewhat, as names are used as the unique id

* And uses Tokio! tasks, mpsc, watch channels, Mutexes, RwLocks, atomics, composite structs...

--Bibek
