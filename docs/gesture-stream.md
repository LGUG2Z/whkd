# The gesture stream

A `swipe3` or `swipe4` binding (a swipe without a direction) is a **stream**. When the fingers land, whkd starts the
binding's command once, and tells it what the fingers do on its **standard input**, one line at a time, until they lift.
When they have, it closes the input.

```
swipe3 : my-program --reads-gestures
```

This is raw touchpad data, in centimetres, for anything that wants it: scrolling a view, scrubbing a timeline, drawing,
changing a volume, driving a window manager. whkd has no idea what the program does with it, and the program needs to
know nothing about whkd beyond this page.

A swipe with a direction (`swipe3left`) is not a stream. It runs its command once, when the swipe happens, and tells
it nothing.

## Format

UTF-8 text, one message per line, each ending in a line feed. A message is a name, then fields separated by single
spaces, each field `key=value`. The fields can be in any order.

```
begin fingers=3 x=4.000 y=2.000 w=10.500 h=7.200 t=118337
move dx=0.420 dy=-0.030 x=4.420 y=1.970 t=118345
move dx=0.510 dy=-0.020 x=4.930 y=1.950 t=118353
move dx=0.470 dy=0.000 x=5.400 y=1.950 t=118361
end x=5.400 y=1.950 t=118369
```

Each line is written and flushed on its own, so a program reading a line at a time sees it as it happens.

| Message | Fields | Meaning |
|---|---|---|
| `begin` | `fingers`, `x`, `y`, `w`, `h`, `t` | Three or four fingers have landed together and stayed. `fingers` is 3 or 4. `x` and `y` are where the middle of the fingers is. `w` and `h` are the size of the touchpad, when it is known |
| `move` | `dx`, `dy`, `x`, `y`, `t` | The fingers moved. `dx` and `dy` are how far the middle of the fingers moved since the last `move` (or since `begin`), and `x` and `y` are where it is now |
| `end` | `x`, `y`, `t` | The fingers lifted. `x` and `y` are where the middle of the fingers last was |
| `cancel` | none | The gesture was abandoned, and there is no `end` for it. A program should put back whatever it was doing, with no settling and no final action |

### Units and directions

- Distances are in **centimetres** on the touchpad, with a decimal point and three decimals, and a leading `-` if
  negative: `0.420`, `-0.030`.
- `x` and `y` are measured from the top left corner of the touchpad. `x` grows to the right, and `y` **downwards**.
- `dx` is therefore positive when the fingers go right, and `dy` positive when they go **down**, towards you.
- `t` is a time in **whole milliseconds**. It is monotonic within a gesture, and only the differences between times
  mean anything: it has no relation to the clock. It comes from the touchpad's own timestamps rather than from when
  the line was written, so the speed worked out from it is the speed of the fingers.

whkd does not work out a velocity, because everything needed to is here: the distance between two `move`s over the
time between them.

### What is guaranteed

- A gesture is a `begin`, any number of `move`s, and exactly one `end` or `cancel`. Then the input is closed.
- `fingers` does not change during a gesture. If it would, the gesture is cancelled, and a new one begins if there are
  still three or four fingers.
- Only movement is reported: a `move` is not written for fingers that are still.
- If the program reads slowly, `move`s can be combined: the `dx` and `dy` of a combined one are the sum, and its `x`,
  `y` and `t` are the latest. `begin`, `end` and `cancel` are never dropped or combined.
- A new gesture starts a new process. Processes do not overlap.

### Compatibility

The format will only grow:

- A program must **ignore** any field it does not know, and any message it does not know.
- A field never changes its meaning or its unit. New information is a new field or a new message.
- A program that wants to know which version of this page it is talking to can look at the environment variable
  `WHKD_GESTURE_PROTOCOL`, which is `1` for this one.

If the input closes with neither an `end` nor a `cancel`, whkd was stopped in the middle of the gesture. A program
should treat that as a `cancel`.

## Reading it

A shell script:

```powershell
while (($line = [Console]::In.ReadLine()) -ne $null) {
    $name, $rest = $line -split ' ', 2
    $fields = @{}
    foreach ($field in $rest -split ' ') { $k, $v = $field -split '=', 2; $fields[$k] = $v }
    switch ($name) {
        'begin'  { "start with $($fields['fingers']) fingers" }
        'move'   { "moved $($fields['dx']) cm across" }
        'end'    { 'done' }
        'cancel' { 'abandoned' }
    }
}
```

Python:

```python
import sys

for line in sys.stdin:
    name, *rest = line.split()
    fields = dict(field.split("=", 1) for field in rest)
    if name == "move":
        print("moved", float(fields["dx"]), "cm across")
```

`docs/gesture-stream.transcript.txt` is the example above as a file, which is what whkd's own tests compare its output
to. A program can copy it as a test of its own.
