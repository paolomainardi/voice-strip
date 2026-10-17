# Presets: references and tuning

This page compares the five presets with published loudness specifications and common voice processing practice, and records what `just test-e2e` measures on them. Use it when you tune a preset.

The values below are the defaults: the Intensity knob at 60 scales EQ, compression, saturation and the expander by 0.6. Loudness targets and the limiter ceiling do not depend on Intensity.

## References

Delivery and loudness specifications:

- **Apple Podcasts** asks for about -16 LKFS with a ±1 dB tolerance and true peaks at or below -1 dBFS, measured with ITU-R BS.1770 ([audio requirements](https://podcasters.apple.com/support/893-audio-requirements)).
- **AES TD1008** recommends -18 LUFS for speech, -16 LUFS for music and -17 LUFS for mixed content in online streaming. Its predecessor TD1004 allowed -16 to -20 LUFS with -1 dBTP ([Telos Alliance summary](https://docs.telosalliance.com/docs/understanding-loudness-for-streaming-audio), [Production Advice](https://productionadvice.co.uk/td1008/)).
- **EBU R 128** sets -23 LUFS and -1 dBTP for broadcast, measured with BS.1770 gating ([R 128 v4](https://tech.ebu.ch/docs/r/r128v4_0.pdf)). The leveler gates the same way.
- **Spotify** normalizes playback to -14 LUFS integrated with -1 dBTP, or -2 dBTP for louder masters. These figures come from secondary sources quoting Spotify ([Softube](https://www.softube.com/se/blog/mastering-for-spotify-why-everything-you-know-is-wrong-and-what-to-do-about-it), [Descript](https://descript.com/blog/article/podcast-loudness-standard-getting-the-right-volume)).
- **ACX audiobooks** are reported to require RMS between -23 and -18 dB, peaks at or below -3 dB and a noise floor at or below -60 dB. These figures come from community sources, not ACX's own page ([HomeBrew Audio](https://homebrewaudio.com/acx-audio-submission-requirements-what-the-heck-do-they-mean), [Audacity forum](https://forum.audacityteam.org/t/meeting-acx-audio-submission-requirements/58706)).
- **ITU-T G.722** defines wideband speech as 50 Hz to 7 kHz, the band that keeps calls intelligible and natural ([G.722](https://en.wikipedia.org/wiki/G.722)).

Processing practice:

- **High-pass filter** around 80 Hz for deeper voices and 100 to 150 Hz for higher ones ([Music Guy Mixing](https://www.musicguymixing.com/voice-eq/), [StreamGeeks](https://streamgeeks.us/how-to-eq-a-podcast/amp)).
- **Proximity effect** adds roughly 6 to 10 dB below 100 Hz with a cardioid microphone very close to the mouth ([Shure SM58 user guide](https://www.mcgill.ca/library/files/library/sm58-user-guide.pdf), [Sound On Sound](https://www.soundonsound.com/techniques/proximity-effect)).
- **Presence** is a broad 2 to 4 dB boost at 3 to 5 kHz; some guides keep it to 1 to 2 dB ([StreamGeeks](https://streamgeeks.us/how-to-eq-a-podcast/amp), [MixingGPT](https://mixinggpt.com/blog/mixing-vocals-cheat-sheet-eq-compression-effects)).
- **Sibilance** sits between 4 and 10 kHz, most often 5 to 7 kHz ([RØDE](https://rode.com/en/blog/all/audio-processing-and-fx-for-podcasting)).
- **Compression** for speech uses 2:1 to 4:1 with a soft knee, a threshold near the average speech level, and 3 to 6 dB of gain reduction on loud syllables ([RØDE](https://rode.com/en/blog/all/audio-processing-and-fx-for-podcasting), [Acast](https://www.acast.com/blog/a-guide-to-vocal-compression-for-podcasters), [Audacity](https://www.audacityteam.org/features/compressor/)).

## Comparison

Measured by `just test-e2e` on LibriSpeech at -26 LUFS with room noise 25 dB below the speech, after the leveler has settled. Compressor GR is the gain reduction exceeded 10% of the time, so roughly what loud syllables get.

| Preset | Target | Reference | High-pass | Presence (3.5 kHz) | Ratio | Compressor GR | Noise removal | Measured loudness |
|---|---|---|---|---|---|---|---|---|
| Broadcast | -16 LUFS | Apple Podcasts -16 | 80 Hz | +3.6 dB | 3.4:1 | 4.6 dB | light | -15.9 LUFS |
| Podcast | -16 LUFS | Apple Podcasts -16 | 90 Hz | +1.8 dB | 2.8:1 | 2.8 dB | light | -15.9 LUFS |
| Call | -18 LUFS | TD1008 speech -18 | 120 Hz | +2.4 dB | 1.6:1 | 0.9 dB | strong | -17.8 LUFS |
| Stream | -14 LUFS | Spotify, YouTube -14 | 90 Hz | +4.2 dB | 4.0:1 | 5.4 dB | medium | -14.2 LUFS |
| Narration | -18 LUFS | TD1008 speech -18, ACX | 70 Hz | +0.3 dB | 1.9:1 | 1.2 dB | off | -17.4 LUFS |

True peaks stay near -2 dBTP, under the -1 dBTP that Apple, TD1008, R 128 and Spotify ask for. Narration limits at -3 dBTP and peaks near -4 dBTP, inside ACX's -3 dB.

Broadcast and Stream compress inside the 3 to 6 dB the guides suggest, Podcast at its lower edge for a natural sound, Call and Narration lightly on purpose. Presence, high-pass and de-esser settings sit inside the referenced ranges.

## Open points

- **Noise in pauses before calibration.** Without calibration the expander assumes a -70 dB noise floor and stays open on ordinary room noise (issue #10). Broadcast and Podcast now remove it with light noise removal by default; Narration, without noise removal, still lets it through.
- **Broadcast is named after radio but targets podcasts.** Its -16 LUFS suits podcast and streaming platforms. Broadcast delivery under R 128 is -23 LUFS, which the preset does not aim at.

## What the end-to-end test checks

`just test-e2e` fails when a preset loses its character, not when it misses an absolute number:

- each preset's loudness is within ±1 LU of its target, with true peaks under the ceiling;
- the compressor works in every preset, and Stream compresses more than Broadcast, Broadcast more than Podcast, Podcast more than Narration;
- Call keeps less low end than Broadcast (its 120 Hz high-pass);
- Stream has more presence than Narration;
- each noise removal level leaves less noise in the pauses than the one below it.

When you retune a preset, update the table above with the new `target/e2e/report.md`.
