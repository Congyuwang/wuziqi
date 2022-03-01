# rust async exercise 多人在线对战五子棋游戏服务器

## 接口API

### 玩家操作

```rust
pub enum Messages {
  /// send user name
  UserName(String),
  /// create a new room
  CreateRoom(SessionConfig),
  /// attempt to join a room with a RoomToken
  JoinRoom(RoomToken),
  /// Quit a room
  QuitRoom,
  /// when in a Room, get ready for a game session
  Ready,
  /// reverse `ready`
  Unready,
  /// play a position in game [0, 15). Out of bounds are ignored.
  /// Repeatedly playing on an occupied position will result in `GameError`.
  Play(u8, u8),
  /// request undo in game.
  RequestUndo,
  /// approve undo requests in game.
  ApproveUndo,
  /// reject undo requests in game.
  RejectUndo,
  /// quit game session (only quit this round).
  QuitGameSession,
  /// chat message
  ChatMessage(String),
  /// exit game (quit game and room), close connection
  /// exiting game without sending `ExitGame` signal is considered `Disconnected`
  ExitGame,
  /// client error: other errors excluding network error
  ClientError(String),
}

```

### 服务器响应

```rust

#[derive(Clone, PartialEq, Debug)]
pub enum RoomState {
  Empty,
  OpponentReady(String),
  OpponentUnready(String),
}

#[derive(Clone, PartialEq, Debug)]
pub enum Responses {
  /// Connection success
  ConnectionSuccess,
  /// Connection Init Error
  ConnectionInitFailure(ConnectionInitError),
  /// response to `CreateRoom`
  RoomCreated(String),
  /// response to `JoinRoom`
  /// the two fields are correspondingly
  /// `room` token
  JoinRoomSuccess(String, RoomState),
  /// response to `JoinRoom`
  JoinRoomFailureTokenNotFound,
  /// response to `JoinRoom`
  JoinRoomFailureRoomFull,
  /// when the other player gets `JoinRoomSuccess`
  /// the `String` is the username
  OpponentJoinRoom(String),
  /// when the other player `QuitRoom`
  OpponentQuitRoom,
  /// when the other player is `Ready`
  OpponentReady,
  /// when the other play does `Unready`
  OpponentUnready,
  /// when both players are `Ready`
  GameStarted(Color),
  /// update field
  FieldUpdate(FieldState),
  /// opponent request undo
  UndoRequest,
  /// undo rejected by timeout
  UndoTimeoutRejected,
  /// undo rejected due to synchronization reason
  UndoAutoRejected,
  /// undo approved
  Undo(FieldStateNullable),
  /// undo rejected by opponent
  UndoRejectedByOpponent,
  /// game session ends, black timeout
  GameEndBlackTimeout,
  /// game session ends, white timeout
  GameEndWhiteTimeout,
  /// game session ends, black wins
  GameEndBlackWins,
  /// game session ends, white wins
  GameEndWhiteWins,
  /// game session ends, draw
  GameEndDraw,
  /// Room score information (player1, player2)
  RoomScores((String, u16), (String, u16)),
  /// opponent quit game session
  OpponentQuitGameSession,
  /// opponent exit game
  OpponentExitGame,
  /// opponent disconnected
  OpponentDisconnected,
  /// game session ends in error
  GameSessionError(String),
  /// ChatMessage: (user_name, message)
  ChatMessage(String, String),
}
```
