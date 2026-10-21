MP3 adresinden (veya yerel ses dosyasından) metin üreten masaüstü uygulaması.
Rust kurmaya gerek yok — aşağıdaki paketi indirip çalıştırmanız yeterli.

| İndirme | Kimin için |
|---|---|
| `sesten-metne-macos-apple-silicon.zip` | M1 / M2 / M3 / M4 Mac'ler (Metal hızlandırmalı) |
| `sesten-metne-macos-intel.zip` | Intel işlemcili Mac'ler |
| `sesten-metne-windows-x64.zip` | 64-bit Windows 10 / 11 |

Hangi Mac'e sahip olduğunuzu bilmiyorsanız:  (Elma menüsü) → "Bu Mac Hakkında".
"Apple M…" yazıyorsa Apple Silicon, "Intel" yazıyorsa Intel sürümünü indirin.

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

### İlk çalıştırmada ne olur

Seçtiğiniz konuşma tanıma modeli (varsayılan `base`, 141 MB) bir kez indirilir
ve önbelleğe alınır; sonraki çalıştırmalar internet gerektirmez. Ses dosyanız
hiçbir sunucuya gönderilmez, her şey bilgisayarınızda işlenir.
