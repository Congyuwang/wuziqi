# Player, Room, and Session

## Virtual Player

This actor wraps around remote connection.
Virtual Player knows which state itself is in.

## Room Manager

Room manager knows the states of rooms.
It accepts `JoinRoom` requests.
It responds Room handlers for Virtual Players to use.

When Virtual Players exit room, they told Room Managers
of their action.
