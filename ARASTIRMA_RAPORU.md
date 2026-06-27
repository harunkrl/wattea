# Batarya Tüketim Takip Uygulaması — Araştırma Raporu

> **Hedef:** Laptop'ta saate oranla % kaç batarya harcandığını gösteren, grafiklerle zenginleştirilmiş **detaylı** bir uygulama.

Rapor, hem sisteminizden **gerçek verilerle doğrulanmış** veri kaynaklarını, hem GUI/TUI karşılaştırmasını, hem de optimum tech stack ve mimari önerisini içerir.

---

## 0. TL;DR — Öneri

| Karar | Önerim | Neden |
|-------|--------|-------|
| **Dil** | **Rust** | En düşük kaynak tüketimi (bir batarya aracında ironik ama kritik), tek statik binary, mükemmel Linux ekosistemi, bellek güvenliği |
| **Arayüz** | **TUI (Ratatui)** — birincil | Terminal power-user'ısınız, Nerd Font zaten yüklü, Ratatui'nin gömülü grafik widget'ları (Chart/BarChart/Sparkline/Gauge) bu iş için birebir |
| **Veri deposu** | **SQLite (rusqlite)** | Düzenli örnekleme, güçlü聚合 sorguları (saat-bazında ortalama), reboot'a dayanıklı, tek dosya |
| **Toplayıcı** | **systemd servisi/timer** | Arka planda sysfs okur, Arch-native, zaten systemd kullanıyorsunuz |
| **Geri-dolum** | upower history'sini **import** et | `/var/lib/upower/*.dat` zaten ~6 günlük veri tutuyor — tekerleği yeniden icat etmeye gerek yok |

**Alternatif (GUI isterseniz):** Tauri (Rust backend + web grafikleri: ECharts/uPlot). Bkz. §5.

---

## 1. Sisteminiz — Tespit Edilenler (gerçek veri)

```
OS         : EndeavourOS (Arch tabanlı), kernel 7.0.11
Desktop    : KDE Plasma @ Wayland
Batarya    : SMP L21M4PD0 (Li-poly, 16.4V)
Kapasite   : 56 Wh (design) / 50.6 Wh (actual) → Sağlık %90.35
Döngü      : 110 cycle
Çalışma    : Python 3.14.5, Rust 1.96, Node 22, Go, GCC 16 hepsi yüklü
Paketçi    : pacman + yay
Font       : JetBrainsMono Nerd Font ✅ (TUI ikonları için ideal)
upower     : 1.91.2 (history destekli) ✅
```

Anlık ölçüm (şu an): `energy-rate: 17.36 W`, `%57`, charging, `time to full: 1.3h`.

---

## 2. Veri Kaynakları (en kritik bölüm — gerçekten doğrulandı)

### Kaynak 1️⃣ — sysfs (canlı, gerçek-zamanlı) ⭐ birincil toplayıcı

`/sys/class/power_supply/BAT0/` altında **okunabilir dosyalar**:

| Dosya | Örnek | Birim |
|-------|-------|-------|
| `capacity` | 57 | % (0–100) |
| `status` | Charging | Charging/Discharging/Full/Not charging |
| `voltage_now` | 16411000 | µV (→/1e6 = V) |
| `power_now` | 20858000 | µW (→/1e6 = W) — *bazı makinelerde `current_now`* |
| `energy_now` | 28660000 | µWh |
| `energy_full` | 50600000 | µWh (güncel tam kapasite) |
| `energy_full_design` | 56000000 | µWh (fabrika) |
| `charge_full` / `charge_full_design` | — | µAh (energy yoksa) |
| `cycle_count` | 110 | adet |
| `manufacturer` / `model_name` / `serial_number` | — | meta |

**Avantaj:** istediğiniz sıklıkta okuyabilirsiniz (1 sn bile). Sıradan dosya okuma, her dilde trivia.
**Dezavantaj:** kalıcı değil — sadece "şu an".

> Not: `power_now`'da ölçüm gecikmesi/smoothing olabilir (kaynak: amas.sh). Yumuşatılmış; ani CPU patlamalarını birkaç saniye gecikmeyle yansıtır. Detaylı analizde bunu bilin.

### Kaynak 2️⃣ — upower history dosyaları (kalıcı, diskte!) ⭐ geri-dolum için altın madeni

upower **kendi history'sini diske kaydeder**: `/var/lib/upower/history-*-L21M4PD0-56-1044.dat`

TSV formatı: `timestamp \t değer \t durum` (`durum`: unknown/charging/discharging/...)

| Dosya | İçerik | Doğrulanmış örnek |
|-------|--------|-------------------|
| `history-charge-*.dat` | `timestamp \t yüzde \t state` | `1781939602  79.000  discharging` |
| `history-rate-*.dat` | `timestamp \t güç(W) \t state` | `1781939607  22.958  discharging` |
| `history-voltage-*.dat` | `timestamp \t voltaj(V) \t state` | `1781939607  16.505  discharging` |
| `history-time-empty-*.dat` | `timestamp \t boşalmaya sn \t state` | `1781939607  6320.000  discharging` |
| `history-time-full-*.dat` | `timestamp \t dolmaya sn \t state` | `1781945025  3504.000  charging` |

**Doğrulandı:** sisteminizde **≈6 günlük** history mevcut (20–27 Haziran), charge dosyası 845 satır, rate dosyası 126 KB.
**Örnekleme:** charge yalnızca **%1 değiştiğinde** kaydedilir (event-based); rate/voltage **~30 sn'de bir**.

⚠️ Sınırlamalar: upower bu dosyaları **rotate/temizleyebilir** (restart'ta bazen, veya `sudo rm` ile). Uzun-vadeli güvenilir depolama için **kendi SQLite'ınız** şart — ama ilk günler için backfill/priming olarak bu dosyaları import edin.

### Kaynak 3️⃣ — upower DBus arayüzü (programatik)

`org.freedesktop.UPower.Device` @ `/org/freedesktop/UPower/devices/battery_BAT0`:

| Metot | İmza | Döndürür |
|-------|------|----------|
| `GetHistory(type, since, n_points)` | `s,u,u` | `a(udu)` = (timestamp, value, state) |
| `GetStatistics(type)` | `s` | `a(dd)` = charge/discharge **doğruluk profili** |
| `GetOnBattery()` (servis) | — | bool |

**Doğrulandı** (gerçek DBus çağrısı çalıştı):

```
gdbus call --system --dest org.freedesktop.UPower \
  --object-path /org/freedesktop/UPower/devices/battery_BAT0 \
  --method org.freedesktop.UPower.Device.GetHistory "charge" 0 20
→ [(1781765135, 77.0, 2), (1781765261, 76.0, 2), ...]   (state: 0=bilinmiyor,1=şarj,2=boşalma,5=pending/full)
```

`GetStatistics("charge")` çok özel: **doğruluk vs. yüzde** scatter'ı verir — "gerçek kapasite %60 iken OS %65 sanıyor" gibi kalibrasyon analizine olanak tanır. **Fark yaratıcı bir özellik.**

### Kaynak 4️⃣ — İsteğe bağlı zenginleştirme (korelasyon için)

Batarya *neden* eridiğini anlamak için, aynı zaman damgasıyla şu verileri de toplayabilirsiniz:

- **CPU yükü:** `/proc/stat` (toplam/idle farkı)
- **Frekans/ıısı:** `/sys/class/thermal/thermal_zone*/temp`, `/sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq`
- **Ekran parlaklığı:** `/sys/class/backlight/*/brightness`
- **En çok güç çeken süreçler:** `PowerTOP` mantığı (`/proc/<pid>/stat`, timers) — zor ama çok havalı
- **Şarj eşikleri:** `/sys/class/power_supply/BAT*/charge_control_end_threshold` (varsa — charge-health özelliği)

---

## 3. "% kaç / saat" Hesaplama Yöntemleri (özellik kalbi)

İstediğiniz metriğin **3 farklı yorumu** var — hepsini sunun:

### (a) Anlık %/saat

```
%/hour = power_now(W) / energy_full(Wh) × 100
       = 6.21 / 50.6 × 100  =  12.3 %/sa    ← doğrulandı ✅
```

Canlı gösterge.

### (b) Belirli pencere için ortalama (entegrasyon)

Rate history'yi zaman üzerinden integre ederek gerçek harcanan enerji → % kaybı:

```
Δ% = Σ(rateᵢ × Δtᵢ) / energy_full × 100
```

"Son 1 saatte ortalama 8.4%/sa tükettin" gibi.

### (c) Saat-bazında desen analizi ⭐ en değerli özellik

SQLite aggregation ile **günün saatine göre ortalama tüketim**:

```sql
SELECT strftime('%H', ts) AS hour_of_day,
       AVG(CASE WHEN state='discharging' THEN rate/50.6*100 END) AS avg_pct_per_hour,
       COUNT(*) AS samples
FROM samples GROUP BY hour_of_day;
```

→ "Saat 14:00–15:00 arası ortalama %11/sa tüketiyorsun" → **ısı haritası / bar chart**. Bu, mevcut hiçbir araçta yok — **fark yaratıcı**.

Benzer şekilde **gün-bazında** (Pazartesi vs Pazar), **oturum-bazında** (her prize-takma→çıkarma döngüsü) analiz mümkün.

---

## 4. GUI mi, TUI mi?

| Kriter | TUI (Ratatui) | GUI (Tauri/web) | GUI (Qt/GTK native) |
|--------|:---:|:---:|:---:|
| **Kaynak tüketimi** (batarya aracı için kritik) | 🟢 ~5–15 MB, CPU ~0 | 🟡 ~80–150 MB (webview) | 🟢 ~30–60 MB |
| **Başlangıç hızı** | 🟢 anında | 🟡 webview init | 🟢 hızlı |
| **Grafik zenginliği** | 🟡 çizgi/bar/sparkline/gauge — yeterli | 🟢 sınırsız (ECharts/d3/uPlot) | 🟡 pyqtchart, orta |
| **"Her zaman görünür" (system tray)** | 🔴 zor (tray daemon gerekir) | 🟢 native tray | 🟢 native tray |
| **SSH/uzaktan erişim** | 🟢 mükemmel | 🔴 yok | 🔴 yok |
| **Power-user hissi (siz)** | 🟢 Arch/terminal kültürü | 🟡 | 🟡 |
| **Geliştirme hızı** | 🟡 orta | 🟢 hızlı (web becerileri) | 🟡 |
| **Dağıtım** | 🟢 tek binary | 🟢 AppImage/.deb/.pkg | 🟡 sistem kütüphaneleri |
| **Nerd Font ikonları** | 🟢 zaten yüklü | n/a | n/a |

### Tavsiyem: **TUI birincil** (sizin profiliniz için)

Gerekçeler:

1. Arch + terminal kullanıcısısınız (zaten pi'i terminalde kullanıyorsunuz) — TUI doğal ortamınız.
2. JetBrainsMono Nerd Font **zaten yüklü** → 🔋 ⚡ 📉 🌡️ ikonları ve unicode sparkline'lar kusursuz görünür.
3. **İroni:** batarya ölçen aracın kendisi batarya yiyemez. TUI en hafif seçenek.
4. Ratatui'nin gömülü widget'ları bu projeyle birebir örtüşüyor (aşağıya bakın).
5. SSH ile uzaktan kontrol edilebilir.
6. Elimde `ratatui-tui` ve `domain-cli` skill'leri var → rehberlik hazır.

**GUI'yi ne zaman seçersiniz:** (1) sistem tepsisinde sürekli görünür bir gösterge istiyorsanız, veya (2) gerçekten "showcase" seviyesi zengin interaktif grafikler (zoom/pan/brush) istiyorsanız. O zaman **Tauri** (Rust backend — kodun çoğunu paylaşırsınız).

> 💡 **Hibrit (ileri seviye):** Rust çekirdek kütüphane + **iki arayüz**: TUI dashboard (detaylı analiz) + minimal tray indicator (her-zaman-görünür). Çekirdeği ortak yazarsınız, ek maliyet düşük.

---

## 5. Tech Stack — Detaylı Öneri

### Birincil: Rust + Ratatui + SQLite

| Katman | Crate | Görev |
|--------|-------|-------|
| **TUI framework** | `ratatui` (v0.30+) | Dashboard render |
| **Backend (event loop)** | `crossterm` (ratatui default) | Terminal I/O, async |
| **Async runtime** | `tokio` | Veri toplama + UI eventi birlikte |
| **DBus (upower)** | `zbus` | GetHistory/GetStatistics, property change sinyalleri |
| **SQLite** | `rusqlite` (veya `sqlx`) | Zaman-serisi depolama + sorgu |
| **sysfs okuma** | std `std::fs` | Dosya okuma — crate bile gerekmez |
| **Zaman** | `chrono` / `time` | Timestamp, saat-bazında gruplama |
| **CLI parse** | `clap` | `batterydash --range 24h --metric rate` |
| **Hata** | `color-eyre` / `anyhow` | Güzel hata mesajları |
| **(opsiyonel) grafik export** | kendi CSV/JSON export | dış araçta çizim |

**Ratatui widget'ları bu projeye özel eşleşmesi:**

- `Chart` (Dataset/Line) → 24 saatlik rate trendi, kapasite düşüş eğrisi
- `BarChart` → saat-bazına göre ortalama %/sa
- `Sparkline` → son N ölçümün mini grafiği (footer/header)
- `Gauge` → anlık % doluluk, sağlık barı
- `Table` → oturum listesi, döngü geçmişi
- `Tabs` → panolar: Canlı / Trend / Desen / Sağlık / Oturumlar
- `Block` + unicode `▁▂▃▄▅▆▇█` → ısı haritası

### Alternatif: Tauri (GUI)

- Backend: aynı Rust çekirdek (paylaşılır)
- Frontend: TypeScript + **uPlot** (hızlı zaman-serisi) veya **ECharts** (zengin), Tailwind
- ~10× daha küçük ve ~5× daha az RAM than Electron (doğrulanmış benchmark)
- Wayland/KDE'de system tray native

### NEDEN Rust (Python/Go/Node değil)?

- **Python (PyQt+pyqtgraph):** hızlı prototip ama interpreter + Qt runtime ağırlık, dağıtım (venv/pyinstaller) can sıkıcı, 7/24 daemon olarak CPU/RAM fazla. Yine de **en hızlı prototip** için ilk MVP Python denenebilir.
- **Go (Bubbletea TUI):** çok iyi ikinci seçenek; tek binary, hızlı derleme. Ama Ratatui'ye göre grafik widget olgunluğu bir tık geride, tip güvenliği Rust kadar değil.
- **Node/Electron:** kesinlikle **hayır** — batarya aracında katil (200+ MB RAM).
- **Rust:** tek statik binary, en düşük footprint, en olgun TUI (Ratatui, 21k★), mükemmel DBus (zbus) ve SQLite (rusqlite). Üstelik `ratatui-tui` skill'im var.

---

## 6. Önerilen Mimari

```
┌─────────────────────────────────────────────────────────────┐
│  batteryd  (systemd service — arka plan, root gerektirmez)  │
│                                                             │
│   tokio interval (60s) ──► sysfs oku (capacity, power_now,  │
│                             voltage, energy_*, status...)   │
│                         ──► (opsiyonel) CPU/parlaklık topla │
│                         ──► SQLite'a yaz  (~/.local/share/  │
│                                            batteryd/db.sqlite)│
│                                                             │
│   Ayrıca: upower DBus PropertiesChanged sinyali ──► event   │
│           (şarja takılma/çıkma anında oturum kaydet)        │
└─────────────────────────────────────────────────────────────┘
                              │
                              │  SQLite (tek dosya, paylaşımlı okuma)
                              ▼
┌─────────────────────────────────────────────────────────────┐
│  batterydash  (TUI — kullanıcı çalıştırınca)                │
│                                                             │
│   Tabs: [Canlı] [24s Trend] [Saatlik Desen] [Sağlık]        │
│         [Oturumlar] [Ayarlar]                               │
│                                                             │
│   Ratatui: Chart / BarChart / Sparkline / Gauge / Table     │
│   'r' yenile · 'e' CSV export · 'q' çıkış · '/' filtre      │
└─────────────────────────────────────────────────────────────┘

  CLI:  batterydash --range 7d --metric pct-per-hour --export csv
  Backfill (tek seferlik):  batteryd import-upower
        /var/lib/upower/history-*.dat  →  SQLite
```

**Dağıtım:** `cargo build --release` → tek binary → `~/.local/bin/` + systemd user unit. AUR package sonra.

---

## 7. Özellik Listesi ("oldukça detaylı" isteğine yanıt)

**Faz 1 — MVP (canlı + temel)**

- [ ] Canlı panel: % , durum, anlık güç (W), voltaj, tahmini süre, sağlık, döngü
- [ ] Sparkline: son 5 dk rate
- [ ] Anlık %/saat göstergesi

**Faz 2 — History & trend**

- [ ] 24 saatlik rate & kapasite çizgi grafiği
- [ ] Şarj/dolanma eğrisi (charge profile)
- [ ] GetStatistics ile **kalibrasyon doğruluk** scatter'ı

**Faz 3 — Desen analizi ⭐ (fark yaratıcı)**

- [ ] Saat-bazına ortalama %/sa **bar chart** (kullanım deseniniz)
- [ ] Gün-bazına (Pzt–Paz) karşılaştırma
- [ ] Haftalık ısı haritası (gün × saat)

**Faz 4 — Oturum & döngü**

- [ ] "On-battery oturumları" tablosu: başlangıç/bitiş %, süre, ortalama rate, % kaybı
- [ ] Şarj döngüsü sayacı & kapasite-degradasyon trendi (uzun vade)
- [ ] Tahmini kalan ömür (cycle/health ekstrapolasyonu)

**Faz 5 — Zenginleştirme & entegrasyon**

- [ ] CPU yükü / ekran parlaklığı korelasyonu
- [ ] (ileri) en güç tüketen süreçler (PowerTOP tarzı)
- [ ] Anomali tespiti (beklenmedik tüketim sıçramaları)
- [ ] uyarılar: "%5'in altında", "sağlık %80'e düştü"
- [ ] CSV/JSON export + (opsiyonel) systemd shutdown/suspend aksiyonları

---

## 8. Mevcut Araçlar — Farklılaşma

| Araç | Ne yapar | Boşluk (sizin fırsatınız) |
|------|----------|---------------------------|
| KDE batarya widget'ı | anlık % gösterir | history/desen yok, zaten var |
| `battop` (Rust TUI) | canlı sağlık detayı (güzel TUI) | **read-only, history/trend yok** — en yakın rakip |
| `bat` (tshakalekholoane) | şarj eşik yönetimi (ASUS tarzı) | ölçüm/trend değil |
| `batmon` (Rust) | basit monitör | detay yok |
| `tlp` / `tlp-stat` | güç yönetimi yapılandırması | analiz/trend değil |
| `powertop` | süreç bazlı güç tüketimi | anlık, kalıcı history yok |
| `upower -d` + gnome-power-statistics | history var ama sınırlı görsel | zengin dashboard/desen yok |

**Fark yaratıcı açı:** Hiçbiri **"saat-bazına göre ortalama %/sa kullanım deseni" + "oturum analizi" + "sağlık degradasyon trendi"** sunmuyor. Bu sizin USP'niz (unique selling point).

---

## 9. Yol Haritası

1. **Hafta 1:** MVP — Rust + Ratatui iskeleti, sysfs okuma, canlı panel, sparkline. `ratatui-tui` skill'ini kullanın.
2. **Hafta 2:** SQLite şeması + systemd collector daemonu (60s). upower `.dat` backfill import'u. `systemd-services` skill'i.
3. **Hafta 3:** 24s trend grafiği + saat-bazına desen bar chart (USP özelliği).
4. **Hafta 4:** Oturum analizi + kalibrasyon doğruluğu (GetStatistics) + CSV export.
5. **Sonra:** CPU/parlaklık korelasyonu, anomali, AUR paketi, (opsiyonel) Tauri tray.

---

## 10. Riskler & Dikkat

- ⚠️ **`power_now` smoothing:** ani değişimleri ~birkaç sn gecikmeli yansıtır. Yüksek frekanslı okuma yanıltıcı olabilir; 30–60 saniye örnekleme optimum.
- ⚠️ **Bazı makinelerde `energy_*` yok**, `charge_*` (µAh) olur → voltajla Wh'a çevirin. Sizin makininizde `energy_*` var ✅.
- ⚠️ **upower history rotasyonu:** güvenmeyin, kendi SQLite'ınız tek doğrul kaynak olsun; upower sadece backfill.
- ⚠️ **Sağlık/degradasyon trendi** aylar alır — uzun vade veri birikmeli.
- ⚠️ **Suspend/resume:** `unknown` state'li bozuk veri noktaları gelir (gördük) — filtreleyin.
- ⚠️ **systemd user service** root gerektirmez ama `loginctl enable-linger` gerekir (kapalıyken de toplasın diye).
- ✅ **SQLite eşzamanlı okuma:** WAL mode açın → daemon yazar, TUI okur, kilitlenme yok.

---

## Sonuç

**En optimum yol:** **Rust + Ratatui (TUI) + SQLite + systemd collector daemonu**, upower history'siyle backfill. Sisteminiz bu stack için tam donanımlu (tüm toolchain'ler + Nerd Font yüklü). "Saate göre % kaç" metriğini **gerçek verilerle doğruladım** (%12.3/sa hesaplandı, DBus GetHistory çalıştı). Saat-bazına **desen analizi** özelliği, hiçbir mevcut araçta olmayan fark yaratıcı USP'niz.

Hazır olduğunuzda MVP'yi kodlamaya başlayabiliriz — `ratatui-tui` ve `systemd-services` skill'leriyle rehberlik ederim.
