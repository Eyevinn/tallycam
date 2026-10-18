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

**Phone.** One row under the status line (in landscape part of the one top
bar), always shown: `1080p30 │ ISO 26 │ 1/100 │ +0.3 EV │ 5200 K +4 │ AF`.
A tap on a value selects its group on a side control; manual values are
yellow, a value the hub just set flashes cyan. Battery, thermal state and tally are in the status row and
the name chip. The design: docs/UI_DESIGN.md.

**Hub.** The same values, in the phone's formats, on one line on every
camera card and shade card (`1080p30 │ ISO 26 │ 1/100 │ +0.3 EV │ 5200 K +4 │
AF`), held values yellow, a change from the phone cyan for 1 s, and CLIP / DARK
after the ISO. A badge on the picture when auto ISO is at a limit or the
picture clips or is too dark; a tag on the name when a camera's mains differs
from the production, or its app is older (protocol 2: "Update app", its
shading controls disabled).

## Not yet

- Gain in dB next to ISO for shaders used to broadcast cameras (6 dB per
  doubling from the lowest ISO).
- Timecode (the phone's disciplined clock) and, when local recording comes,
  storage remaining.

## How others adjust

- **Filmic Pro.** Version 6 had arc sliders under the thumbs (exposure on
  one side, focus on the other). Version 7 replaced them with two vertical
  sliders: the left one for ISO, shutter, both, or zoom (selectable), the
  right one for focus distance.
- **Final Cut Camera (Apple).** Exposure is automatic with an EV
  compensation dial; manual has ISO, shutter speed and shutter angle.
  Manual focus is a dial; a double tap on the dial resets it. Touch and
  hold a subject locks focus and exposure (a yellow AE/AF Lock), a tap
  anywhere releases it.
- **Blackmagic Camera.** A single tap on a value in the top bar opens its
  adjustment; manual focus is a slider, with tap to focus.
- **iPhone Camera Control (iPhone 16 and later).** A light press shows one
  control (zoom or exposure), a slide on the button changes it, a double
  light press picks another control. Apps can add their own controls
  (AVFoundation's capture controls). Reviewers find the slide imprecise and
  easy to hit by accident.

What TallyCam takes from them: two vertical controls under the thumbs
(Filmic Pro 7), the value as the way in (Blackmagic), double tap to reset
and touch and hold for AE/AF lock (Final Cut Camera). It keeps a rate
rocker instead of a position slider: a slider jumps where the thumb lands,
a rocker only ever moves the value smoothly, which is what a live picture
needs (the broadcast zoom rocker works the same way).

## Sources

- [Blackmagic Camera](https://www.blackmagicdesign.com/products/blackmagiccamera)
- [Broadcast: Blackmagic unveils iPhone camera app](https://www.broadcastnow.co.uk/tech/ibc2023-blackmagic-unveils-iphone-camera-app/5185963.article)
- [SmallHD Cine 7 with Sony VENICE control kit](https://hotrodcameras.com/collections/all/products/smallhd-7-cine-7-touchscreen-monitor-with-sony-venice-camera-control-kit)
- [Sony RCP-1001](https://pro.sony/en_EE/pdf/rcp-1001)
- [SKAARHOJ RCP Pro](https://skaarhoj.com/product/rcp-pro)
- [Cyanview](https://cyanview.com/)
- [Cyanview tally](https://support.cyanview.com/docs/Configuration/Tally)
- [Filmic Pro v7 redesigned UI (CineD)](https://www.cined.com/filmic-pro-v7-launched-redesigned-ui-custom-function-button-and-more/)
- [Filmic Pro v6 (learnaboutfilm)](https://learnaboutfilm.com/?p=10084)
- [Final Cut Camera: exposure and white balance (Apple)](https://support.apple.com/guide/final-cut-camera/deve37e999e1/ios)
- [Final Cut Camera: camera settings (Apple)](https://support.apple.com/guide/final-cut-camera/dev0eb85c508)
- [Blackmagic Camera tech specs](https://blackmagicdesign.com/products/blackmagiccamera/techspecs)
- [Use the Camera Control on iPhone (Apple)](https://support.apple.com/en-afri/guide/iphone/iph0c397b154/ios)
- [Enhancing your app experience with the Camera Control (Apple Developer)](https://developer.apple.com/tutorials/data/documentation/avfoundation/enhancing-your-app-experience-with-the-camera-control.md)
