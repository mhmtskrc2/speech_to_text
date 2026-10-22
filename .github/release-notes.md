MP3 adresinden (veya yerel ses dosyasından) metin üreten masaüstü uygulaması.
Rust kurmaya gerek yok — aşağıdaki paketi indirip çalıştırmanız yeterli.

| İndirme | Kimin için |
|---|---|
| `sesten-metne-macos.zip` | Tüm Mac'ler (Apple Silicon ve Intel tek pakette) |
| `sesten-metne-windows-x64.zip` | 64-bit Windows 10 / 11 |

Mac paketi evrensel (universal) bir uygulamadır; hangi işlemciye sahip
olduğunuzu bilmenize gerek yok. Apple Silicon Mac'lerde Metal hızlandırması
devrededir.

### macOS'ta ilk açılış

Uygulama imzalı olmadığı için macOS ilk seferde engeller. Çözümü:

1. Zip'i açın, **Sesten Metne.app**'i `Uygulamalar` klasörüne sürükleyin.
2. Uygulamaya **sağ tıklayın** → **Aç** → çıkan uyarıda yine **Aç**.

Bunu yalnızca bir kez yapmanız gerekir. Hiç açılmazsa, Terminal'de:

```bash
xattr -dr com.apple.quarantine "/Applications/Sesten Metne.app"
```

### Windows'ta ilk açılış

Zip'i açıp **Sesten Metne.exe**'yi çalıştırın. SmartScreen uyarı verirse
**Ek bilgi** → **Yine de çalıştır** deyin (uygulama imzalı değil).

Açılmazsa [Microsoft Visual C++ Redistributable](https://aka.ms/vs/17/release/vc_redist.x64.exe)
kurulu olmayabilir; kurup tekrar deneyin.

### Hız hakkında

Model seçimi süreyi doğrudan belirler. Apple Silicon bir Mac'te ölçülen,
**1 saatlik ses** için yaklaşık süreler:

| Model | Apple Silicon (Metal) | Yalnızca CPU |
|---|---|---|
| `base` | ~2 dakika | ~7 dakika |
| `large-v3-turbo (q5_0)` | ~5 dakika | ~26 dakika |
| `large-v3` | ~9 dakika | ~44 dakika |

(63 saniyelik Türkçe bir kayıtla ölçülüp bir saate oranlandı; model zaten
indirilmiş kabul edilmiştir. Kendi makineniz ve ses kalitenize göre değişir.)

Mac paketinde Metal hazır gelir. Windows paketi CPU'da çalışır, yani sağdaki
sütun geçerlidir. `large-v3` en doğru sonucu verir ama en yavaşıdır; günlük
kullanımda `large-v3-turbo (q5_0)` doğruluk/hız dengesi olarak çok daha iyidir.

İlerleme çubuğunun yanında geçen süre ve tahmini kalan süre gösterilir, yani
yavaş bir işi askıda kalmış bir işten ayırt edebilirsiniz.

### İlk çalıştırmada ne olur

Seçtiğiniz konuşma tanıma modeli (varsayılan `base`, 141 MB) bir kez indirilir
ve önbelleğe alınır; sonraki çalıştırmalar internet gerektirmez. Ses dosyanız
hiçbir sunucuya gönderilmez, her şey bilgisayarınızda işlenir.
