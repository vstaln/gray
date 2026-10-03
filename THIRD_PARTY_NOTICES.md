# Third-party notices

gray itself is MIT (see `LICENSE`). Cargo dependencies are covered by
`Cargo.lock`, not listed here.

`reference/` is gitignored (0 tracked files): study input only, never
shipped. The rows below cover what gray's sources admit to deriving
from an upstream ("ported from" headers, protocol notes). Anything
else is design parity in gray's own words per
`docs/reference-study-protocol.md`.

## Apache-2.0 — openai/codex

Upstream: https://github.com/openai/codex (Apache-2.0).

Copyright (c) OpenAI. Original portions (c) their authors under
Apache-2.0; gray's changes are gray's own under MIT (see `LICENSE`).
The Apache-2.0 license text is included below; the Windows release zip
also carries this file next to `LICENSE`.

| gray | upstream file / area | relationship |
|---|---|---|
| `crates/gray-core/src/compact.rs` + `agent_compact.rs` (via `crates/gray/src/compact/`; old `compact_v2.rs` port deleted) | `codex-rs/core/src/compact_remote*.rs` @ `1fb5158b` (constants `RETAINED_MESSAGE_TOKEN_BUDGET`, `MAX_RETAINED_AGENT_MESSAGE_TOKENS` mirrored verbatim) + `@howaboua/pi-codex-conversion` guidance shape (MIT, (c) 2026 Igor Warzocha) | port-turned-parity: pipeline rewritten in gray's own shape, constants + guidance kept |
| `crates/gray/src/term_keys.rs` | `codex-rs/tui/src/tui/keyboard_modes.rs` | port: Kitty enhancement flags + Ghostty/iTerm/tmux degrade rules, own wording |
| `crates/gray/src/composer/text_area.rs`, `crates/gray-core/src/event.rs`, `crates/gray-core/src/agent.rs`, `crates/gray-core/src/agent_loop.rs`, `crates/gray-provider/src/openai.rs`, `crates/gray/src/repl/format.rs`, `crates/gray/src/print.rs` | codex TUI / core | design parity: studied, rewritten in gray's own shape |

## MIT — NousResearch/hermes-agent

Upstream: https://github.com/NousResearch/hermes-agent (MIT,
Copyright (c) 2025 Nous Research). Permission notice preserved by
this row; no verbatim upstream text ships in gray.

| gray | upstream area | relationship |
|---|---|---|
| `crates/gray/src/gateway/` (`mod.rs` daemon shape, `pid.rs` claim doctrine) | hermes gateway craft (pid claim, control socket, supervisor, cron tick) | port of the doctrine (O_EXCL claim, /proc starttime, socket-is-liveness), own code |
| `crates/gray-plugin/src/capabilities.rs` | `plugin_capabilities.py` (one id per enforcing surface) | port of the capability-id scheme, own consent/audit layer |
| `crates/gray/src/gateway/progress.rs` (removed; was: turn-progress rows) | hermes gateway progress craft | was a port; file deleted, row kept for history |

## MIT — badlogic/pi-mono, prime-agent, OnlyTerp/toolrush, sst/opencode

| gray | upstream | license | relationship |
|---|---|---|---|
| `crates/gray/src/compact/`, `crates/gray/src/composer/input/` (paste-collapse rule) | https://github.com/badlogic/pi-mono (MIT, Copyright (c) 2025 Mario Zechner) and `reference/prime-agent` (MIT, Copyright (c) 2025 Mario Zechner, (c) 2026 Prime Intellect) | MIT | parity: collapse rule (>10 lines or >1000 chars), own code |
| `crates/gray-core/src/parallel.rs` (parallel batch lane; `MAX_BATCH`/`MAX_WORKERS` constants parity, no verbatim code) | https://github.com/OnlyTerp/toolrush (MIT, Copyright (c) 2026 OnlyTerp) | MIT | constants parity only |
| `crates/gray/src/repl/attachments.rs`, `crates/gray/src/repl/format.rs`, skills-dir interop | https://github.com/sst/opencode (MIT, Copyright (c) 2025 opencode) | MIT | parity targets, own code |

## GPL-3.0 — study only, nothing shipped

| gray | upstream | license |
|---|---|---|
| `docs/openhuman-reference-study.md` (study only — no gray code informed by it yet) | https://github.com/tinyhumansai/openhuman (+ 18 vendored `tinyhumansai/*` engine crates) | GPL-3.0 |

OpenHuman and its vendored engine crates are GPL-3.0, so nothing from
them may be ported into gray (MIT): the study is design parity only,
per `docs/reference-study-protocol.md`.

---

## Appendix: Apache License 2.0

```
                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      Use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS
```
