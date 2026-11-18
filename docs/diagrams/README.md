# Diagrams

Two maps of routwatch, in English and Arabic. Each map comes as a self-contained
HTML file and as a still PNG preview of the same drawing.

Every file was regenerated with Archify 3.2 from commit `48ed795444182e048b7d2f69b62c9f66967e97fb` (2026-11-18),
and the claims drawn inside the HTML carry the source line they were taken from at that revision.
The architecture map names the shared time axis and the panel layer, and the sequence map adds a fourth stage: a guarded
message delete, one request per index, verified by re-reading the inbox.
Nothing here is hand-drawn, and nothing here describes a device that was tested:
the diagrams describe the code, and the screenshots they reference are the same
synthetic fixtures used in the README.

## Files

| File | What it is | Language |
|---|---|---|
| `routwatch-architecture.html` | Architecture map: crates, modules and the data path from the CPE API to the TUI, the headless modes and the report writers | Arabic |
| `routwatch-architecture.en.html` | The same architecture map | English |
| `routwatch-sequence.html` | Sequence map: one fetch cycle, from the worker thread through the HTTP client and the parser back to the UI | Arabic |
| `routwatch-sequence.en.html` | The same sequence map | English |
| `routwatch-architecture.png` | Still preview of the architecture map | Arabic |
| `routwatch-architecture.en.png` | Still preview of the architecture map | English |
| `routwatch-sequence.png` | Still preview of the sequence map | Arabic |
| `routwatch-sequence.en.png` | Still preview of the sequence map | English |

## How to open them

The HTML files open locally: download the file and open it in any browser. There
is no server, no install and no account. CSS, JavaScript and themes are embedded
in each file, so it works offline.

The PNG files are still previews for places that cannot run HTML, such as a chat
message or a slide.

Archify embeds an em dash (U+2014) in its own interface strings. After every
regeneration the four pages are normalized to a hyphen, because the project
check rejects that character in docs.

---

# المخططات

خريطتان لـ routwatch، بالعربية والإنجليزية. كل خريطة تأتي كملف HTML قائم بذاته
وكصورة PNG ثابتة لنفس الرسم.

كل ملف أُعيد توليده بـArchify 3.2 من الالتزام `48ed795444182e048b7d2f69b62c9f66967e97fb` (2026-11-18)، والأدلة
المكتوبة داخل ملف HTML مرتبطة بأسطر الكود التي أُخذت منها.
مخطط المعمارية يسمّي محور الوقت الموحّد وطبقة اللوحات، ومخطط التسلسل يضيف مرحلة رابعة: حذف رسالة
بعد تأكيد، طلب واحد لكل فهرس، ويُتحقَّق منه بقراءة صندوق الوارد مرة أخرى. لا شيء هنا مرسوم
يدويًا، ولا شيء هنا يصف جهازًا جرى اختباره: المخططات تصف الكود، واللقطات التي
تشير إليها هي نفس الـ fixtures الاصطناعية المستخدمة في README.

## الملفات

| الملف | ما هو | اللغة |
|---|---|---|
| `routwatch-architecture.html` | مخطط المعمارية: الحِزم والوحدات ومسار البيانات من واجهة الراوتر إلى الواجهة الطرفية، وأوضاع headless وكتّاب التقارير | العربية |
| `routwatch-architecture.en.html` | نفس مخطط المعمارية | الإنجليزية |
| `routwatch-sequence.html` | مخطط التسلسل: دورة جلب واحدة، من خيط العامل عبر عميل HTTP والمُحلِّل وعودةً إلى الواجهة | العربية |
| `routwatch-sequence.en.html` | نفس مخطط التسلسل | الإنجليزية |
| `routwatch-architecture.png` | معاينة ثابتة لمخطط المعمارية | العربية |
| `routwatch-architecture.en.png` | معاينة ثابتة لمخطط المعمارية | الإنجليزية |
| `routwatch-sequence.png` | معاينة ثابتة لمخطط التسلسل | العربية |
| `routwatch-sequence.en.png` | معاينة ثابتة لمخطط التسلسل | الإنجليزية |

## كيف تُفتح

ملفات HTML تُفتح محليًا: نزّل الملف وافتحه في أي متصفح. لا خادم، ولا تثبيت، ولا
حساب. الـ CSS والـ JavaScript والثيمات مضمّنة داخل كل ملف، فيعمل بلا إنترنت.

ملفات PNG معاينات ثابتة للأماكن التي لا تشغّل HTML، مثل رسالة محادثة أو شريحة
عرض.
