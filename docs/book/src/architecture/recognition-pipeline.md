# Recognition pipeline and the lighting-dependence limitation

This page explains, end-to-end, how Facegate turns a camera frame into a
"match / no match" decision — and **why that pipeline is sensitive to
ambient lighting in a way Windows Hello is not**. It is the technical
counterpart to the user-facing FAQ entries on low light and re-enrolment.

If you only remember one thing: Facegate today does **identity matching on
the RGB stream with an RGB-trained model**. Windows Hello does identity
matching on the **IR stream with an IR-trained model**, paired with an
**active IR illuminator** that floods the face with its own light source.
The illuminator is what makes Windows Hello insensitive to ambient
lighting — not the camera, not the model in isolation, but the combination
of "active light source + sensor + model all in the same spectrum".

## How Facegate authenticates today

```text
        ┌────────────────────────────────────────────────────────────────┐
        │                       facegate-brokerd                         │
        │                                                                │
  ┌─────┴────┐   raw frame    ┌────────┐   crop+landmarks   ┌─────────┐  │
  │  V4L2    │ ─────────────► │ SCRFD  │ ─────────────────► │ Align   │  │
  │  RGB     │   (640×360,    │ detect │   (5-point lm)     │ 112×112 │  │
  │  camera  │    YUYV→RGB)   └────────┘                    └────┬────┘  │
  └──────────┘                                                   │       │
        ▲                                                        ▼       │
        │ ambient light                                  ┌───────────┐   │
        │ shapes the                                     │  ArcFace  │   │
        │ pixel values                                   │ embedder  │   │
        │ at every stage                                 │ (RGB!)    │   │
        │                                                └─────┬─────┘   │
        │                                                      │         │
        │                                                      ▼         │
        │                                                ┌───────────┐   │
        │                                                │  cosine   │   │
        │                                                │  vs each  │   │
        │                                                │  template │   │
        │                                                └─────┬─────┘   │
        │                                                      │         │
        │                                                      ▼         │
        │                                                  matched?      │
        │                                                  (≥ threshold) │
        └────────────────────────────────────────────────────────────────┘
```

Source map: V4L2 capture lives in `facegate_core::camera`, SCRFD in
`facegate_core::detection`, alignment + embedding in
`facegate_core::embedding` (`align_face` → ArcFace ONNX session), match
decision in `facegate_brokerd::main` (`handle_match_frame`).

### Where lighting hits the pipeline

Ambient light is not "noise on top of a clean signal" — it shapes the
pixel values at every stage of the chain above:

| Stage | What ambient light changes |
|---|---|
| Sensor exposure / gain | Auto-exposure picks a different exposure time and gain. The same scene at two light levels produces frames with different noise floors, different white balance, and different effective dynamic range. |
| SCRFD detection | At low contrast SCRFD's bbox + 5-point landmarks become less stable. A landmark drift of a few pixels propagates into a worse alignment, then a worse embedding. |
| Alignment | Same as above — the similarity transform is fit to whatever SCRFD returned. Garbage in, garbage out. |
| ArcFace embedding | The 512-d vector ArcFace emits is *not* invariant to lighting. The same identity under two markedly different lightings produces two vectors with cosine similarity meaningfully lower than the same identity under the same lighting. |

So a template enrolled in well-lit conditions sits at one point in the
512-d embedding space; the same user captured in a dim room sits
somewhere else. If the threshold is tight enough to keep impostors out,
it is often tight enough to keep the same-user-different-room match out
too.

This is **the** reason re-enrolling in each new environment "fixes" the
problem: it just adds new templates that happen to be close to the
runtime captures. It is a workaround, not a solution.

## How Windows Hello sidesteps this

Windows Hello uses near-infrared (NIR, ~850 nm) hardware that ships in
two parts on the same camera module:

- a **passive IR sensor** that reads only the NIR band, and
- an **active IR illuminator** (the row of faint LEDs you can spot
  with your phone's camera near the lens) that floods the face with a
  known, constant NIR light pulse synchronised with the capture.

```text
                    ┌──────────────────┐
                    │  IR illuminator  │   active source
                    │  (~850 nm LEDs)  │   ── floods the face
                    └────────┬─────────┘      with constant NIR light
                             │
                             ▼
                       ┌──────────┐
                       │   face   │
                       └─────┬────┘
                             │ NIR reflectance
                             ▼
                    ┌──────────────────┐
                    │  IR sensor       │   sees mostly the
                    │  (~850 nm band)  │   illuminator's light,
                    │  GREY / Y8 output│   not the room's
                    └────────┬─────────┘
                             │
                             ▼
                    ┌──────────────────┐
                    │ IR-trained face  │   model has learned
                    │ embedder         │   identity from NIR
                    │ (Microsoft prop.)│   crops directly
                    └────────┬─────────┘
                             │
                             ▼
                          matched?
```

The point is *not* "IR is better than RGB". The point is that the
**illuminator dominates the scene's NIR brightness**, so the sensor sees
roughly the same image whether the room is sunlit, lamp-lit, or pitch
dark. The face's NIR reflectance properties (skin, eyes, hair) barely
change with ambient light. Pair that with a model that was trained on
NIR crops in the first place, and the embedding becomes
*environmentally invariant* in a way that no RGB pipeline can match.

A side effect that helps anti-spoofing: printed photos and most screens
do not reflect 850 nm light the way real skin does, so the IR sensor
sees something visibly off before the matcher even runs.

## Why Facegate cannot just "switch to IR"

The IR stream is already wired up in v0.3.0 — the optional
`[camera.ir]` section + `[camera.cross_check]` enables a parallel IR
capture. Today it is used **only as a liveness signal**: the broker
runs SCRFD on it to confirm a face is present and spatially aligns with
the RGB face, and stops there. Identity matching never sees the IR
embedding.

This was not always the case. An earlier version of the broker ran
ArcFace on the IR crop and compared the RGB and IR embeddings directly.
It was changed (commit `1582696`) because:

> *"The previous cross-check ran ArcFace on the IR crop and compared the
> RGB/IR embeddings, which rejected nearly every genuine user because
> the embedder was never trained on cross-modal pairs."*

The ArcFace weights Facegate ships were trained on RGB faces. Running
them on IR crops produces near-random similarity scores, because the
features the model has learned to look at (skin colour, fine RGB
texture, micro-contrast around the eyes) are not present in an 8-bit
greyscale NIR frame in the same form.

To get Windows-Hello-style behaviour we therefore need a *different
model*, trained on IR — and that model is what the open-source
ecosystem does not yet provide under a licence Facegate can ship. See
the [roadmap entry on IR-native recognition][rm-ir] for the audit
behind that statement.

## Where the project is heading

The work to close the lighting-dependence gap is split into three
explicit pieces, each with its own GitHub issue:

| # | Title | What it does | Why it is needed |
|---|---|---|---|
| [#51][issue-51] | Recognition robustness: guided sample variation + illumination preprocessing | Adds CLAHE-style normalisation to the aligned face crop before ArcFace, and changes the enrolment UX to actively prompt the user to vary lighting/pose/distance between samples. Bumps the default sample count from 3 to 5. | The cheap robustness wins. Does not require changing the model. Lands first so users get a usable experience and so any later work has an honest baseline. |
| [#52][issue-52] | Replace InsightFace-bundled models with permissively-licensed alternatives | Drops the `buffalo_l.zip` download (InsightFace, non-commercial-only) and switches to AuraFace-v1 (Apache-2.0) for the embedder and YuNet (MIT) for detection. | Independent licence-hygiene work. Required so any benchmarking done after #51/#16 lands is against the embedder we will actually ship long-term. |
| [#16][issue-16] | IR recognition pipeline: empirical evaluation + interchangeable backends | After #51 and #52 land, evaluate whether the IR camera path can deliver useful identity matching when run through the *post-#52* embedder with the *post-#51* preprocessing. Land a trait-based backend abstraction so future swaps don't touch call sites. | Without a real open-source IR model, the only honest path is to empirically test the existing one on IR through the new preprocessing. If it works, ship an `ir-primary` profile. If not, document why and open a long-term issue for custom model training. |

The order matters: **#51 and #52 must both land before #16 produces
benchmark numbers anyone should trust**. See each issue's "Blocked by"
header for the dependency wiring.

## Honest comparison vs Windows Hello

|  | Windows Hello (today) | Facegate (today) | Facegate (post-#51 + #52 + #16) |
|---|---|---|---|
| Primary identity sensor | IR | RGB | RGB primary, IR mode pending #16 evaluation |
| IR illuminator used | Yes, active and synchronised | Passive (sensor only, no driver-side illuminator control on most laptops) | Same as today |
| Embedder training spectrum | NIR | RGB (InsightFace ArcFace) | RGB (AuraFace, Apache-2.0) |
| Embedder licence | Proprietary | Non-commercial only | Apache-2.0 |
| Illumination preprocessing | Implicit (active illuminator) | None | CLAHE on luminance channel |
| Enrolment guidance | Single-pose with motion prompts | Single condition, multi-sample prompts but no variation guidance | Multi-sample with explicit variation prompts |
| Lighting robustness | Effectively independent of ambient light | Sensitive — needs re-enrolment per environment | Substantially better via #51; close to Windows Hello only if #16 IR path proves out |

The gap that remains after #51 + #52 + #16 (assuming #16 finds the IR
path is not usable through an RGB-trained embedder) is **a custom
IR-native model**. That is a research/training project, not a feature,
and it will get its own milestone if and when the empirical work in #16
shows it is the only remaining path.

[issue-16]: https://github.com/me02329/facegate/issues/16
[issue-51]: https://github.com/me02329/facegate/issues/51
[issue-52]: https://github.com/me02329/facegate/issues/52
[rm-ir]: ../security/roadmap.md#ir-native-and-multi-modal-recognition
