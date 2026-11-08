# Offline Pinyin Dictionary

`dictionary.tsv` is a normalized derivative of the Android Open Source Project
PinyinIME dictionary. It contains all 65,105 source character readings and phrases for
offline candidate lookup. This is input-method data, not a book archive or an
upstream application API response.

The generated set contains 17,037 single-character readings and 48,068 phrase
readings. Its UTF-8 data occupies 1,327,401 bytes. Uncommon phrases may require
choosing their component characters rather than a whole-phrase candidate.

## Source and license

- Project: [AOSP PinyinIME](https://android.googlesource.com/platform/packages/inputmethods/PinyinIME/).
- Inspected revision: `49aebad1c1cfbbcaa9288ffed5161e79e57c3679`.
- Source file: [`jni/data/rawdict_utf16_65105_freq.txt`](https://android.googlesource.com/platform/packages/inputmethods/PinyinIME/+/49aebad1c1cfbbcaa9288ffed5161e79e57c3679/jni/data/rawdict_utf16_65105_freq.txt).
- Copyright: 2009, The Android Open Source Project.
- License: Apache License, Version 2.0. The complete upstream NOTICE, including
  its license text, is retained verbatim in `LICENSE-AOSP.txt`.

The dictionary license applies to this bundled data. It does not choose the
application's project license.

## Changes from the source

`generate_dictionary.py` performs these deterministic changes:

1. Decode the source UTF-16 data and export UTF-8 TSV.
2. Keep CJK phrases, including the ideographic zero character, regardless of
   the source's extended GBK flag. Every source entry meets this condition.
3. Join syllables into lowercase, toneless ASCII keys; encode umlaut-u as `v`.
4. Deduplicate identical spelling/phrase pairs using their highest frequency.
5. Retain all source entries; the bundled dictionary has no artificial
   frequency cutoff or application-specific ranking adjustment.
6. Replace raw frequency with its global descending-frequency rank.
7. Sort by spelling, rank, and phrase for deterministic lookup and diffs.

The transformation does not copy the Android decoder or JNI implementation.
Candidate ranking is based on the source's historical unigram frequencies;
this is not a modern adaptive or predictive language model.

## Data format

Each line has three tab-separated fields:

```text
joined_toneless_pinyin<TAB>phrase<TAB>rank
```

`rank` is a positive integer; a lower number is a more frequent source entry.
Different pronunciations of a character may appear on different lines. A
candidate engine should support component-by-component entry when a whole
name or phrase is not present, and should not silently discard unmatched
composition.

## Reproduction

After obtaining the exact source revision above, run the following in the
designated remote development environment:

```sh
python3 assets/pinyin/generate_dictionary.py \
  /path/to/PinyinIME/jni/data/rawdict_utf16_65105_freq.txt \
  assets/pinyin/dictionary.tsv
```

Only the transformed dictionary and required notice are bundled. Android's
decoder, binary dictionary, JNI code, and other source files are not vendored.

## Recorded source integrity and historical remote checks

This directory was imported from the local WeRead implementation working tree
on 2026-11-03. The historical checks below belong to that reference project;
they have not been rerun for BiliComics. The dictionary, generator, and original
AOSP notice were copied without changing their bytes. See
[reference provenance](../../docs/REFERENCE_PROVENANCE.md).

- Source SHA-256: `408700f28a56091fa07f3b849a0f134fbfc71e6b2ae9b3f52973a5b076f599ff`.
- Output SHA-256: `a32775ccf82e47d04feb3f01e5174ec6fcf12a15064da14cb5cd840a1c1eb422`.
- Checks on `test-env`: exactly 65,105 entries, unique spelling/phrase pairs,
  lowercase ASCII keys, positive ranks, and deterministic sort order passed.
- Representative exact candidates include the standard Chinese words for
  China (`zhongguo`), hello (`nihao`), reading (`dushu`), and fiction
  (`xiaoshuo`).

These are dictionary transformation checks, not BiliComics verification,
Kindle hardware, upstream availability, or complete input-method acceptance.
