# Session Syncing

## Role Syncing During Play

Sync Rule:

- `not-my-turn` after sending `play`
- `my-turn` after receiving `response` of opponent color

```mermaid
sequenceDiagram
    participant b as Black Player
    participant g as Game Session
    participant w as White Player
    Note over w: not my turn
    activate w
    Note over b: my turn
    b ->> g: play(color=black)
    Note over b: not my turn
    g -->> b: reponse(color=black)
    Note over b: my color
    g -->> w: reponse(color=black)
    Note over w: opponent color
    Note over w: my turn
    deactivate w
```

## Syncing Undo

Rules:

- `ban Undo` after sending `Undo request`: can only send once.
- `allow Undo` after receiving `resposne` of my color: do not allow `Undo` if my action is not yet received.
- `auto reject` Undo when `not my turn`: when opponent has sent `Play`, but I have not yet received.
- `ban Undo` after receiving `response` of opponent color: opponent has played.
- `ban Undo` after being `approved`, `auto rejected` or `rejected`: cannot apply once.

### Case 1: White Player has not yet played

```mermaid
sequenceDiagram
    participant b as Black Player
    participant g as Game Session
    participant w as White Player
    Note over w: not my turn
    activate w
    Note over b: my turn
    Note over b: ban Undo
    b ->> g: play(color=black)
    Note over b: not my turn
    g -->> b: reponse(color=black)
    Note over b: my color
    Note over b: allow Undo
    g -->> w: reponse(color=black)
    Note over w: opponent color
    Note over w: my turn
    deactivate w
    Note over w: ban Undo
    b ->> g: request Undo
    Note over b: RequestingUndo
    Note over b: ban Undo
    g -->> w: request Undo
    Note over w: ApprovingUndo
    Note over w: Start Request Timer
    alt approve
    w -->> g: approve Undo
    Note over w: not my turn
    g -->> b: approve Undo
    Note over b: my turn
    g -->> w: response
    else reject
    w -->> g: reject Undo
    g -->> b: reject Undo
    Note over b: ban Undo
    else timeup
    w -->> g: timeup-reject Undo
    g -->> b: timeup-reject Undo
    Note over b: ban Undo
    end
```

### Case 2: White Player has played, but Undo arrived first

```mermaid
sequenceDiagram
    participant b as Black Player
    participant g as Game Session
    participant w as White Player
    Note over w: not my turn
    activate w
    Note over b: my turn
    Note over b: ban Undo
    b ->> g: play(color=black)
    Note over b: not my turn
    Note over b: allow Undo
    g -->> b: reponse(color=black)
    Note over b: my color
    g -->> w: reponse(color=black)
    Note over w: opponent color
    Note over w: my turn
    deactivate w
    Note over w: ban Undo
    b ->> g: request Undo
    Note over b: RequestingUndo
    w ->> g: play(color=White)
    Note over w: not my turn 
    g -->> w: request Undo
    w -->> g: auto reject
    g -->> b: auto reject
    Note over b: ban Undo
    g -->> b: reponse(color=white)
    Note over b: opponent color
    Note over b: my turn
    Note over b: ban Undo
    g -->> w: reponse(color=white)
    Note over w: my color
    Note over w: allow Undo
```
