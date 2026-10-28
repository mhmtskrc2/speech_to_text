MP3 adresinden (veya yerel ses dosyasından) metin üreten masaüstü uygulaması.
Rust kurmaya gerek yok — aşağıdaki paketi indirip çalıştırmanız yeterli.

| İndirme | Kimin için |
|---|---|
| `Sesten-Metne.exe` | 64-bit Windows 10 / 11 |
| `sesten-metne-macos` | Tüm Mac'ler — evrensel ikili |

Her platform için tek dosya var ve ikisinde de GPU hızlandırma hazır gelir:
Mac'te Metal, Windows'ta Vulkan (NVIDIA / AMD / Intel). Seçenek yok, kullanılabilir
bir aygıt varsa kullanılır; yoksa işlem kendiliğinden CPU'da çalışır.

### Windows'ta ilk açılış

Uygulama `vulkan-1.dll`'e ihtiyaç duyar; bu dosya
ekran kartı sürücüsüyle birlikte gelir ve güncel sürücüsü olan her makinede
bulunur. Varlığını şöyle kontrol edebilirsiniz — komut isteminde:

```
dir C:\Windows\System32\vulkan-1.dll
```

"File Not Found" diyorsa ekran kartı sürücünüzü güncelleyin (NVIDIA / AMD /
Intel'in kendi sitesinden); sürücü bu dosyayı da kurar. SmartScreen uyarısı
çıkarsa **Ek bilgi** → **Yine de çalıştır** deyin.

### macOS'ta ilk açılış

İndirilen dosya tek bir çalıştırılabilir ikilidir. Tarayıcı indirmede
çalıştırma iznini düşürdüğü ve dosya imzalı olmadığı için, Terminal'de bir kez
şunları çalıştırın:

```bash
chmod +x ~/Downloads/sesten-metne-macos
xattr -d com.apple.quarantine ~/Downloads/sesten-metne-macos
```

Sonra dosyaya çift tıklayarak ya da Terminal'den çalıştırabilirsiniz.

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

Mac dosyasında Metal, Windows GPU dosyasında Vulkan hazır gelir — yani her
ikisinde de soldaki sütuna yakın süreler beklenir (Vulkan, Metal kadar hızlı
olmayabilir). Sağdaki sütun, uygun bir aygıt bulunamayıp işlemin CPU'ya
düştüğü durum içindir.

Windows'taki Vulkan hızı ölçülmedi: derleme sunucularında ekran kartı yok,
dolayısıyla yalnızca ikilinin Vulkan'a bağlandığı doğrulanabildi.

`large-v3` en doğru sonucu verir ama en yavaşıdır; günlük kullanımda
`large-v3-turbo (q5_0)` doğruluk/hız dengesi olarak çok daha iyidir.

İlerleme çubuğunun yanında geçen süre ve tahmini kalan süre gösterilir, yani
yavaş bir işi askıda kalmış bir işten ayırt edebilirsiniz.

### İlk çalıştırmada ne olur

Seçtiğiniz konuşma tanıma modeli bir kez indirilir ve önbelleğe alınır;
sonraki çalıştırmalar internet gerektirmez. Varsayılan `large-v3` en doğru
sonucu verir ama **2.9 GB** indirir — hemen başlamak için menüden daha küçük
bir model (örneğin `base`, 141 MB) seçebilirsiniz.

Ses dosyanız hiçbir sunucuya gönderilmez, her şey bilgisayarınızda işlenir.
