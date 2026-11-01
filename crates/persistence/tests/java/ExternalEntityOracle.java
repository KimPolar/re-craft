import java.io.*;
import java.nio.file.*;
import java.util.Arrays;
import net.minecraft.SharedConstants;
import net.minecraft.nbt.*;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.storage.*;

public class ExternalEntityOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Path directory=Path.of(args[1]).resolve("dimensions/minecraft/overworld/entities");
        try(var region=new RegionFile(new RegionStorageInfo("parity",null,"entities"),
                directory.resolve("r.0.0.mca"),directory,RegionFileVersion.VERSION_DEFLATE,false)) {
            if(args[0].equals("absent")) {
                try(var in=region.getChunkDataInputStream(new ChunkPos(1,0))) {
                    if(in!=null) throw new AssertionError("deleted entity chunk remains readable");
                }
                if(Files.exists(directory.resolve("c.1.0.mcc"))) throw new AssertionError("deleted external chunk remains");
                System.out.println("OK");
                return;
            }
            CompoundTag chunk;
            try(var in=region.getChunkDataInputStream(new ChunkPos(0,0))) {
                if(in==null) throw new AssertionError("entity chunk missing");
                chunk=NbtIo.read((DataInput)in);
            }
            if(chunk.getIntOr("DataVersion",0)!=5023 || chunk.getLongOr("future",0)!=77) throw new AssertionError("metadata lost");
            var entity=chunk.getListOrEmpty("Entities").getCompoundOrEmpty(0);
            byte[] payload=entity.getByteArray("payload").orElseThrow();
            if(args[0].equals("verify")) {
                if(!Arrays.equals(payload,Files.readAllBytes(Path.of(args[2])))) throw new AssertionError("payload differs");
            } else if(args[0].equals("rewrite")) {
                payload[0]^=0x5a; entity.putByteArray("payload",payload);
                try(var out=region.getChunkDataOutputStream(new ChunkPos(0,0))) { NbtIo.write(chunk,(DataOutput)out); }
            } else throw new IllegalArgumentException(args[0]);
        }
        if(!Files.isRegularFile(directory.resolve("c.0.0.mcc"))) throw new AssertionError("external chunk missing");
        System.out.println("OK");
    }
}
