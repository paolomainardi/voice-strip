# Voice Strip for the Omarchy bar

A bar widget for [Omarchy](https://omarchy.org/) that shows Voice Strip's state and controls it from a compact faceplate.

- **Bar icon:** a microphone with a small live output meter while processing, a red crossed-out microphone while muted, and a dimmed microphone in bypass or while the app is closed.
- **Left click:** opens the faceplate with the input and output VU meters, gain reduction, loudness, presets, noise removal, headphones and the Intensity knob.
- **Middle click:** turns processing on or off.
- **Right click:** opens the Voice Strip window, and starts the app when it is not running.
- **START** in the panel runs the app in the background, without its window. Closing the window later also leaves the processing running; QUIT in the panel or Quit in the tray ends it.
- **Scroll:** changes the Intensity.

The panel follows the app: the meters use the same ballistics as the window, and the type is the app's own (Oswald and IBM Plex Mono).

## Install

1. Put `voice-strip` on your `PATH`, for example:

   ```sh
   cargo build --release -p app
   install -m 755 target/release/voice-strip ~/.local/bin/
   ```

2. Link the plugin and place it on the bar. From the repository, `just omarchy-plugin` does both steps:

   ```sh
   ln -s "$PWD/extras/omarchy/io.github.paolomainardi.voicestrip" ~/.config/omarchy/plugins/
   omarchy-shell shell rescanPlugins
   omarchy plugin enable io.github.paolomainardi.voicestrip
   ```

## Settings

| Setting                         | Default       | What it does                                                                                |
| ------------------------------- | ------------- | ------------------------------------------------------------------------------------------- |
| Command                         | `voice-strip` | The executable, or its full path.                                                           |
| Panel look                      | Faceplate     | Faceplate is the app's standard view, Console its advanced view, Omarchy follows the theme. |
| Show the level meter            | on            | Shows the output level next to the icon while processing.                                   |
| Hide when Voice Strip is closed | off           | Shows the icon only while the app runs.                                                     |

## Keys in the panel

| Key                        | Action                    |
| -------------------------- | ------------------------- |
| Space or Enter             | Processing on or off      |
| M                          | Mute or unmute            |
| 1 to 5                     | Broadcast to Narration    |
| N                          | Next noise removal level  |
| Left and Right, or H and L | Intensity down or up by 5 |
| P                          | Headphones on or off      |
| W                          | Open the window           |
| Esc                        | Close the panel           |

## From scripts and key bindings

The widget keeps one connection to the app's control socket and reads its state as JSON lines, so it does not poll. The same commands work from a shell:

```sh
voice-strip ctl start           # run the app in the background if it is not running
voice-strip ctl status          # state as JSON
voice-strip ctl toggle          # processing on or off
voice-strip ctl preset call
voice-strip ctl noise strong
omarchy-shell io.github.paolomainardi.voicestrip toggle    # the panel
omarchy-shell io.github.paolomainardi.voicestrip power     # processing on or off
omarchy-shell io.github.paolomainardi.voicestrip mute      # mute or unmute
```

Run `voice-strip ctl --help` for the full list.
