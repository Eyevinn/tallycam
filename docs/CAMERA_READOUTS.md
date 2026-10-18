# Camera readouts

What camera and shading interfaces usually show, and what TallyCam shows on
the phone and in the hub. Research from October 2026.

## What others show

**Camera apps and cinema cameras (the operator).** The Blackmagic Camera app
on iPhone puts a status bar along the top: lens, resolution, frame rate,
shutter, iris, ISO, white balance and tint, timecode and storage remaining,
with the histogram, audio meters and function buttons at the side. Tapping an
indicator opens its adjustment (tap ISO to change ISO); a swipe hides the
whole display. Cinema cameras and their monitors (Sony VENICE through a
SmallHD control kit, for example) show the same set: frame rate, shutter,
colour temperature, ND, timecode, media and battery status.

**Remote control panels (the shader).** A broadcast RCP shows each value it
controls next to its knob or on its screen: iris as an F number (CLS when
closed) and master black on Sony's RCP-1001; iris and tally on the joystick
display of SKAARHOJ's RCP Pro, which also covers shutter, ISO, gain, white
balance, tint and RGB; gain, black, gamma and ND on Cyanview's shading
screen, where the camera name's background shows the tally colour. One panel
serves several cameras and follows the one selected.

Common to both: the values are always visible, a value that is set by hand
stands out from one that is automatic, the tally colour sits on the camera's
name, and the value itself is where you go to change it.

## What TallyCam shows

**Phone.** One line at the bottom centre, toggled in the aids menu:
`1080p30 · ISO 26 · 1/100 · +0.3 EV · 5200 K · AF`. A value in a manual or
locked group is yellow. EV only when not 0, the bitrate stays in the status
line. On a narrow screen focus goes first, then white balance. Battery,
thermal state and tally are in the status line and the name chip.

**Hub.** The same line on every camera card and shade card, with a badge
when auto ISO is at a limit (too bright, too dark).

## Not yet

- Tap a value in the readout to open its control (as the Blackmagic app
  does), which could replace the EXP, WB and FOC chips.
- Gain in dB next to ISO for shaders used to broadcast cameras (6 dB per
  doubling from the lowest ISO).
- Timecode (the phone's disciplined clock) and, when local recording comes,
  storage remaining.

## Sources

- [Blackmagic Camera](https://www.blackmagicdesign.com/products/blackmagiccamera)
- [Broadcast: Blackmagic unveils iPhone camera app](https://www.broadcastnow.co.uk/tech/ibc2023-blackmagic-unveils-iphone-camera-app/5185963.article)
- [SmallHD Cine 7 with Sony VENICE control kit](https://hotrodcameras.com/collections/all/products/smallhd-7-cine-7-touchscreen-monitor-with-sony-venice-camera-control-kit)
- [Sony RCP-1001](https://pro.sony/en_EE/pdf/rcp-1001)
- [SKAARHOJ RCP Pro](https://skaarhoj.com/product/rcp-pro)
- [Cyanview](https://cyanview.com/)
- [Cyanview tally](https://support.cyanview.com/docs/Configuration/Tally)
